package testutil

import (
	"context"
	"os"
	"path/filepath"
	"strconv"
	"sync"
	"testing"

	"github.com/haierkeys/fast-note-sync-service/internal/config"
	"github.com/haierkeys/fast-note-sync-service/internal/dao"
	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/pkg/writequeue"
	"go.uber.org/zap"
	"gorm.io/gorm"
)

type WorkspaceEnv struct {
	UID           int64
	OtherUID      int64
	MainDB        *gorm.DB
	BlobRoot      string
	Dao           *dao.Dao
	WorkspaceRepo domain.WorkspaceRepository

	t          testing.TB
	writeQueue *writequeue.Manager
	userDBMu   sync.Mutex
	userDBs    map[int64]*gorm.DB
}

func NewWorkspaceEnv(t testing.TB) *WorkspaceEnv {
	t.Helper()
	tempDir := t.TempDir()
	dbConfig := config.DatabaseConfig{
		Type: "sqlite",
		Path: filepath.Join(tempDir, "database", "main.sqlite3"),
	}
	if err := os.MkdirAll(filepath.Dir(dbConfig.Path), 0755); err != nil {
		t.Fatalf("create workspace test database directory: %v", err)
	}

	logger := zap.NewNop()
	mainDB, err := dao.NewEngine(dbConfig, logger)
	if err != nil {
		t.Fatalf("open workspace test database: %v", err)
	}
	writeQueue := writequeue.New(nil, logger)
	daoInstance := dao.New(
		mainDB,
		context.Background(),
		dao.WithConfig(&dbConfig),
		dao.WithUserDatabaseConfig(&dbConfig),
		dao.WithLogger(logger),
		dao.WithWriteQueueManager(writeQueue),
	)

	env := &WorkspaceEnv{
		UID:           41,
		OtherUID:      42,
		MainDB:        mainDB,
		BlobRoot:      filepath.Join(tempDir, "workspace-blobs"),
		Dao:           daoInstance,
		WorkspaceRepo: dao.NewWorkspaceRepository(daoInstance),
		t:             t,
		writeQueue:    writeQueue,
		userDBs:       make(map[int64]*gorm.DB),
	}
	t.Cleanup(env.cleanup)
	env.UserDB(env.UID)
	return env
}

func (e *WorkspaceEnv) UserDB(uid int64) *gorm.DB {
	e.t.Helper()
	e.userDBMu.Lock()
	defer e.userDBMu.Unlock()

	if db, ok := e.userDBs[uid]; ok {
		return db
	}
	db := e.Dao.ResolveDB("user_workspace_" + strconv.FormatInt(uid, 10))
	if db == nil {
		e.t.Fatalf("open routed workspace test database for uid %d", uid)
	}
	e.userDBs[uid] = db
	return db
}

func (e *WorkspaceEnv) cleanup() {
	if err := e.writeQueue.Shutdown(context.Background()); err != nil {
		e.t.Errorf("shut down workspace test write queue: %v", err)
	}

	e.userDBMu.Lock()
	userDBs := make([]*gorm.DB, 0, len(e.userDBs))
	for _, db := range e.userDBs {
		userDBs = append(userDBs, db)
	}
	e.userDBMu.Unlock()
	for _, db := range userDBs {
		closeWorkspaceTestDB(e.t, "user", db)
	}
	closeWorkspaceTestDB(e.t, "main", e.MainDB)
}

func closeWorkspaceTestDB(t testing.TB, name string, db *gorm.DB) {
	sqlDB, err := db.DB()
	if err != nil {
		t.Errorf("get %s workspace test database handle: %v", name, err)
		return
	}
	if err := sqlDB.Close(); err != nil {
		t.Errorf("close %s workspace test database: %v", name, err)
	}
}
