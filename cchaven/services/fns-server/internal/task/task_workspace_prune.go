package task

import (
	"context"
	"time"

	"github.com/haierkeys/fast-note-sync-service/internal/app"
	"go.uber.org/zap"
)

// WorkspacePruneTask expires stale workspace operations and reclaims replay/blob storage.
type WorkspacePruneTask struct {
	app       *app.App
	logger    *zap.Logger
	listUIDs  func(context.Context) ([]int64, error)
	pruneUser func(context.Context, int64, time.Time) error
}

func (t *WorkspacePruneTask) Name() string {
	return "WorkspacePrune"
}

func (t *WorkspacePruneTask) LoopInterval() time.Duration {
	return time.Hour
}

func (t *WorkspacePruneTask) IsStartupRun() bool {
	return true
}

func (t *WorkspacePruneTask) Run(ctx context.Context) error {
	if t == nil {
		return nil
	}
	listUIDs := t.listUIDs
	if listUIDs == nil {
		if t.app == nil || t.app.UserRepo == nil {
			return nil
		}
		listUIDs = t.app.UserRepo.GetAllUIDs
	}
	pruneUser := t.pruneUser
	if pruneUser == nil {
		if t.app == nil || t.app.WorkspaceSyncService == nil {
			return nil
		}
		pruneUser = t.app.WorkspaceSyncService.PruneUser
	}
	uids, err := listUIDs(ctx)
	if err != nil {
		return err
	}
	now := time.Now().UTC()
	var firstErr error
	for index, uid := range uids {
		if err := ctx.Err(); err != nil {
			return err
		}
		if err := pruneUser(ctx, uid, now); err != nil {
			if firstErr == nil {
				firstErr = err
			}
			if t.logger != nil {
				t.logger.Error("workspace prune failed", zap.Int64("uid", uid), zap.Error(err))
			}
		}
		if index+1 < len(uids) {
			timer := time.NewTimer(25 * time.Millisecond)
			select {
			case <-ctx.Done():
				if !timer.Stop() {
					<-timer.C
				}
				return ctx.Err()
			case <-timer.C:
			}
		}
	}
	return firstErr
}

func NewWorkspacePruneTask(appContainer *app.App) (Task, error) {
	return &WorkspacePruneTask{
		app:    appContainer,
		logger: appContainer.Logger(),
		listUIDs: func(ctx context.Context) ([]int64, error) {
			return appContainer.UserRepo.GetAllUIDs(ctx)
		},
		pruneUser: func(ctx context.Context, uid int64, now time.Time) error {
			return appContainer.WorkspaceSyncService.PruneUser(ctx, uid, now)
		},
	}, nil
}

func init() {
	RegisterWithApp(func(appContainer *app.App) (Task, error) {
		return NewWorkspacePruneTask(appContainer)
	})
}
