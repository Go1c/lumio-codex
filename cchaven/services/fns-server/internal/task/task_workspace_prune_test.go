package task

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/haierkeys/fast-note-sync-service/internal/app"
	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/service"
	"github.com/stretchr/testify/require"
	"go.uber.org/zap"
)

func TestWorkspacePruneTaskPrunesEveryActiveUser(t *testing.T) {
	userRepo := &workspacePruneUserRepository{uids: []int64{42, 7}}
	syncService := &workspacePruneSyncService{}
	container := &app.App{
		Repositories: &app.Repositories{UserRepo: userRepo},
		Services:     &app.Services{WorkspaceSyncService: syncService},
	}
	task := &WorkspacePruneTask{app: container, logger: zap.NewNop()}

	err := task.Run(context.Background())
	require.NoError(t, err)
	require.Equal(t, []int64{42, 7}, syncService.uids)
	require.Len(t, syncService.times, 2)
	require.WithinDuration(t, time.Now(), syncService.times[0], time.Second)
	require.WithinDuration(t, syncService.times[0], syncService.times[1], time.Second)
}

func TestWorkspacePruneTaskVisitsUsersInOrderAndContinuesAfterOneError(t *testing.T) {
	var visited []int64
	wantErr := errors.New("one user failed")
	task := &WorkspacePruneTask{
		logger: zap.NewNop(),
		listUIDs: func(context.Context) ([]int64, error) {
			return []int64{42, 7, 9}, nil
		},
		pruneUser: func(_ context.Context, uid int64, _ time.Time) error {
			visited = append(visited, uid)
			if uid == 7 {
				return wantErr
			}
			return nil
		},
	}

	err := task.Run(context.Background())

	require.ErrorIs(t, err, wantErr)
	require.Equal(t, []int64{42, 7, 9}, visited)
}

func TestWorkspacePruneTaskStopsPromptlyOnCancellation(t *testing.T) {
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	var visited []int64
	task := &WorkspacePruneTask{
		logger: zap.NewNop(),
		listUIDs: func(context.Context) ([]int64, error) {
			return []int64{42, 7}, nil
		},
		pruneUser: func(_ context.Context, uid int64, _ time.Time) error {
			visited = append(visited, uid)
			cancel()
			return nil
		},
	}

	err := task.Run(ctx)

	require.ErrorIs(t, err, context.Canceled)
	require.Equal(t, []int64{42}, visited)
}

type workspacePruneUserRepository struct {
	domain.UserRepository
	uids []int64
}

func (r *workspacePruneUserRepository) GetAllUIDs(context.Context) ([]int64, error) {
	return append([]int64(nil), r.uids...), nil
}

type workspacePruneSyncService struct {
	service.WorkspaceSyncService
	uids  []int64
	times []time.Time
}

func (s *workspacePruneSyncService) PruneUser(_ context.Context, uid int64, now time.Time) error {
	s.uids = append(s.uids, uid)
	s.times = append(s.times, now)
	return nil
}

var _ service.WorkspaceSyncService = (*workspacePruneSyncService)(nil)
