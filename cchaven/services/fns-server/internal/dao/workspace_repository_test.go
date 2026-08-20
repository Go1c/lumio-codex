package dao_test

import (
	"context"
	"encoding/hex"
	"errors"
	"fmt"
	"math"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/haierkeys/fast-note-sync-service/internal/dao"
	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/internal/model"
	"github.com/haierkeys/fast-note-sync-service/internal/testutil"
	"github.com/stretchr/testify/require"
	"github.com/zeebo/blake3"
	"gorm.io/gorm"
	"gorm.io/gorm/clause"
)

const (
	workspaceRepositoryWorkspaceID = "10000000-0000-4000-8000-000000000002"
	workspaceRepositoryClientID    = "10000000-0000-4000-8000-000000000001"
	workspaceRepositoryOperationID = "10000000-0000-4000-8000-000000000004"
	workspaceRepositoryConflictID  = "10000000-0000-4000-8000-000000000005"
)

var workspaceRepositoryTableNames = []string{
	"workspace",
	"workspace_blob",
	"workspace_blob_ref",
	"workspace_client",
	"workspace_conflict",
	"workspace_event",
	"workspace_operation",
	"workspace_path",
}

type workspaceRepositorySchemaSnapshot struct {
	Columns    map[string]workspaceRepositoryColumnSnapshot
	PrimaryKey []string
	Indexes    []string
}

type workspaceRepositoryColumnSnapshot struct {
	DatabaseType string
	DeclaredType string
	Length       int64
	Nullable     bool
}

var workspaceRepositoryExpectedSchema = map[string]workspaceRepositorySchemaSnapshot{
	"workspace": {
		Columns: map[string]workspaceRepositoryColumnSnapshot{
			"id": workspaceRepositoryColumn("integer", false), "workspace_id": workspaceRepositorySizedColumn("varchar", 36, false),
			"global_revision": workspaceRepositoryColumn("integer", false), "replay_floor_revision": workspaceRepositoryColumn("integer", false),
			"live_path_count": workspaceRepositoryColumn("integer", false), "live_bytes": workspaceRepositoryColumn("integer", false),
			"created_at": workspaceRepositoryColumn("datetime", false), "updated_at": workspaceRepositoryColumn("datetime", false),
		},
		PrimaryKey: []string{"id"},
		Indexes:    []string{"idx_workspace_workspace_id|true|workspace_id"},
	},
	"workspace_client": {
		Columns: map[string]workspaceRepositoryColumnSnapshot{
			"workspace_id": workspaceRepositorySizedColumn("varchar", 36, false), "client_id": workspaceRepositorySizedColumn("varchar", 36, false),
			"last_ack_revision": workspaceRepositoryColumn("integer", false), "last_seen_at": workspaceRepositoryColumn("datetime", false),
		},
		Indexes: []string{"idx_workspace_client_identity|true|workspace_id,client_id"},
	},
	"workspace_path": {
		Columns: map[string]workspaceRepositoryColumnSnapshot{
			"id": workspaceRepositoryColumn("integer", false), "workspace_id": workspaceRepositorySizedColumn("varchar", 36, false),
			"path_key": workspaceRepositorySizedColumn("char", 64, false), "path": workspaceRepositoryColumn("text", false),
			"path_revision": workspaceRepositoryColumn("integer", false), "kind": workspaceRepositorySizedColumn("varchar", 16, false),
			"content_hash": workspaceRepositorySizedColumn("varchar", 71, true), "size": workspaceRepositoryColumn("integer", false),
			"modified_at_ms": workspaceRepositoryColumn("integer", false), "executable": workspaceRepositoryColumn("numeric", false),
			"tombstone": workspaceRepositoryColumn("numeric", false),
		},
		PrimaryKey: []string{"id"},
		Indexes: []string{
			"idx_workspace_path_identity|true|workspace_id,path_key",
			"idx_workspace_path_revision|false|workspace_id,path_revision",
		},
	},
	"workspace_event": {
		Columns: map[string]workspaceRepositoryColumnSnapshot{
			"id": workspaceRepositoryColumn("integer", false), "workspace_id": workspaceRepositorySizedColumn("varchar", 36, false),
			"revision": workspaceRepositoryColumn("integer", false), "kind": workspaceRepositorySizedColumn("varchar", 24, false), "operation_id": workspaceRepositorySizedColumn("varchar", 36, false),
			"origin_client_id": workspaceRepositorySizedColumn("varchar", 36, false), "mutation_json": workspaceRepositoryColumn("text", false),
			"path_state_json": workspaceRepositoryColumn("text", false), "old_path_state_json": workspaceRepositoryColumn("text", true),
			"new_path_state_json": workspaceRepositoryColumn("text", true), "resolved_json": workspaceRepositoryColumn("text", true), "created_at": workspaceRepositoryColumn("datetime", false),
		},
		PrimaryKey: []string{"id"},
		Indexes: []string{
			"idx_workspace_event_created_at|false|workspace_id,created_at",
			"idx_workspace_event_revision|true|workspace_id,revision",
		},
	},
	"workspace_operation": {
		Columns: map[string]workspaceRepositoryColumnSnapshot{
			"workspace_id": workspaceRepositorySizedColumn("varchar", 36, false), "client_id": workspaceRepositorySizedColumn("varchar", 36, false),
			"operation_id": workspaceRepositorySizedColumn("varchar", 36, false), "request_kind": workspaceRepositorySizedColumn("varchar", 32, false),
			"request_digest": workspaceRepositorySizedColumn("char", 64, false), "state": workspaceRepositorySizedColumn("varchar", 16, false),
			"result_action": workspaceRepositorySizedColumn("varchar", 64, true), "result_json": workspaceRepositoryColumn("text", true), "conflict_json": workspaceRepositoryColumn("text", true),
			"required_hash": workspaceRepositorySizedColumn("varchar", 71, true), "conflict_revision": workspaceRepositorySizedColumn("varchar", 20, true),
			"expires_at": workspaceRepositoryColumn("datetime", true), "created_at": workspaceRepositoryColumn("datetime", false),
			"updated_at": workspaceRepositoryColumn("datetime", false),
		},
		Indexes: []string{"idx_workspace_operation_identity|true|client_id,operation_id"},
	},
	"workspace_conflict": {
		Columns: map[string]workspaceRepositoryColumnSnapshot{
			"workspace_id": workspaceRepositorySizedColumn("varchar", 36, false), "conflict_id": workspaceRepositorySizedColumn("varchar", 36, false),
			"conflict_revision": workspaceRepositorySizedColumn("varchar", 20, false), "path_key": workspaceRepositorySizedColumn("char", 64, false),
			"path": workspaceRepositoryColumn("text", false), "kind": workspaceRepositorySizedColumn("varchar", 24, false),
			"status": workspaceRepositorySizedColumn("varchar", 16, false), "ancestor_json": workspaceRepositoryColumn("text", false),
			"current_json": workspaceRepositoryColumn("text", false), "incoming_json": workspaceRepositoryColumn("text", false), "rename_target_json": workspaceRepositoryColumn("text", true),
			"created_by_operation_id": workspaceRepositorySizedColumn("varchar", 36, false),
			"resolution_operation_id": workspaceRepositorySizedColumn("varchar", 36, true), "resolution_revision": workspaceRepositoryColumn("integer", true),
			"resolution_choice": workspaceRepositorySizedColumn("varchar", 16, true), "resolution_path_state_json": workspaceRepositoryColumn("text", true),
			"resolved_by_client_id": workspaceRepositorySizedColumn("varchar", 36, true), "resolved_at": workspaceRepositoryColumn("datetime", true),
			"created_at": workspaceRepositoryColumn("datetime", false), "updated_at": workspaceRepositoryColumn("datetime", false),
		},
		Indexes: []string{
			"idx_workspace_conflict_id|true|conflict_id",
			"idx_workspace_conflict_status|false|workspace_id,status,updated_at",
		},
	},
	"workspace_blob": {
		Columns: map[string]workspaceRepositoryColumnSnapshot{
			"content_hash": workspaceRepositorySizedColumn("varchar", 71, false), "size": workspaceRepositoryColumn("integer", false),
			"utf8_valid": workspaceRepositoryColumn("numeric", false), "ref_count": workspaceRepositoryColumn("integer", false),
			"unreferenced_at": workspaceRepositoryColumn("datetime", true), "created_at": workspaceRepositoryColumn("datetime", false),
			"updated_at": workspaceRepositoryColumn("datetime", false),
		},
		PrimaryKey: []string{"content_hash"},
	},
	"workspace_blob_ref": {
		Columns: map[string]workspaceRepositoryColumnSnapshot{
			"id": workspaceRepositoryColumn("integer", false), "content_hash": workspaceRepositorySizedColumn("varchar", 71, false),
			"owner_type": workspaceRepositorySizedColumn("varchar", 16, false), "owner_key": workspaceRepositorySizedColumn("varchar", 128, false),
			"created_at": workspaceRepositoryColumn("datetime", false), "updated_at": workspaceRepositoryColumn("datetime", false),
		},
		PrimaryKey: []string{"id"},
		Indexes: []string{
			"idx_workspace_blob_ref_content_hash|false|content_hash",
			"idx_workspace_blob_ref_owner|true|owner_type,owner_key,content_hash",
		},
	},
}

func TestWorkspaceRepositoryUsesOneUserDatabaseAndLeavesMainDBUntouched(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	require.NoError(t, env.MainDB.Exec("CREATE TABLE main_workspace_sentinel (id INTEGER PRIMARY KEY, value TEXT NOT NULL)").Error)
	require.NoError(t, env.MainDB.Exec("INSERT INTO main_workspace_sentinel (id, value) VALUES (1, 'unchanged')").Error)
	mainTablesBefore := workspaceRepositoryTables(t, env.MainDB)

	require.NoError(t, env.WorkspaceRepo.Migrate(context.Background(), env.UID))

	userDB := env.Dao.ResolveDB("user_workspace_41")
	require.NotNil(t, userDB)
	require.Same(t, userDB, env.UserDB(env.UID))
	for _, table := range workspaceRepositoryTableNames {
		require.Truef(t, userDB.Migrator().HasTable(table), "routed database is missing table %q", table)
		require.Falsef(t, env.MainDB.Migrator().HasTable(table), "main database unexpectedly contains table %q", table)
	}
	require.Equal(t, mainTablesBefore, workspaceRepositoryTables(t, env.MainDB))

	var sentinelValue string
	require.NoError(t, env.MainDB.Raw("SELECT value FROM main_workspace_sentinel WHERE id = 1").Scan(&sentinelValue).Error)
	require.Equal(t, "unchanged", sentinelValue)
}

func TestWorkspaceRepositoryRejectsNonPositiveUIDBeforeRouting(t *testing.T) {
	for _, uid := range []int64{0, -1} {
		t.Run(fmt.Sprintf("uid_%d", uid), func(t *testing.T) {
			env := testutil.NewWorkspaceEnv(t)
			ctx := context.Background()
			mainTablesBefore := workspaceRepositoryTables(t, env.MainDB)
			routedDBsBefore := len(env.Dao.KeyDb)

			require.EqualError(t, env.WorkspaceRepo.Migrate(ctx, uid), "workspace uid must be positive")

			var readCalled atomic.Bool
			require.EqualError(t, env.WorkspaceRepo.Read(ctx, uid, func(domain.WorkspaceReadTx) error {
				readCalled.Store(true)
				return nil
			}), "workspace uid must be positive")
			require.False(t, readCalled.Load())

			var writeCalled atomic.Bool
			require.EqualError(t, env.WorkspaceRepo.Write(ctx, uid, func(domain.WorkspaceWriteTx) error {
				writeCalled.Store(true)
				return nil
			}), "workspace uid must be positive")
			require.False(t, writeCalled.Load())

			require.Equal(t, mainTablesBefore, workspaceRepositoryTables(t, env.MainDB))
			require.Len(t, env.Dao.KeyDb, routedDBsBefore)
			_, routed := env.Dao.KeyDb["user_workspace_"+fmt.Sprint(uid)]
			require.False(t, routed)
		})
	}
}

func TestWorkspaceRepositoryAutoMigrateIsIdempotent(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	ctx := context.Background()
	require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
	userDB := env.UserDB(env.UID)
	before := workspaceRepositorySchema(t, userDB)
	require.Equal(t, workspaceRepositoryTableNames, workspaceRepositoryTables(t, userDB))
	require.Equal(t, workspaceRepositoryExpectedSchema, before)
	workspaceRepositoryRequireCheckConstraint(
		t,
		userDB,
		&model.WorkspaceOperation{},
		"chk_workspace_operation_state",
		"check (state in ('waiting_blob','terminal','expired_guard'))",
	)
	workspaceRepositoryRequireCheckConstraint(
		t,
		userDB,
		&model.WorkspaceBlobRef{},
		"chk_workspace_blob_ref_owner_type",
		"check (owner_type in ('path','event','conflict'))",
	)

	require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
	require.NoError(t, dao.NewWorkspaceRepository(env.Dao).Migrate(ctx, env.UID))

	require.Equal(t, before, workspaceRepositorySchema(t, userDB))
}

func TestWorkspaceRepositorySupports4096BytePathThroughPathKey(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	ctx := context.Background()
	require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
	path, err := dto.ParseWorkspacePath(strings.Repeat("a", 4096))
	require.NoError(t, err)

	require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		if err := tx.CreateWorkspace(workspaceRepositoryWorkspace(0)); err != nil {
			return err
		}
		return tx.SavePath(domain.WorkspacePathRecord{
			WorkspaceID:  workspaceRepositoryWorkspaceID,
			Path:         path,
			PathRevision: 1,
			Kind:         dto.WorkspaceEntryFile,
			Size:         3,
			ModifiedAtMS: 1,
		})
	}))

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		got, err := tx.Path(workspaceRepositoryWorkspaceID, path)
		if err != nil {
			return err
		}
		require.Equal(t, path, got.Path)
		return nil
	}))

	var stored model.WorkspacePath
	require.NoError(t, env.UserDB(env.UID).
		Where("workspace_id = ?", workspaceRepositoryWorkspaceID).
		First(&stored).Error)
	sum := blake3.Sum256([]byte(path))
	require.Equal(t, hex.EncodeToString(sum[:]), stored.PathKey)
	require.Len(t, stored.PathKey, 64)
	require.Equal(t, string(path), stored.Path)
}

func TestWorkspaceRepositoryRejectsPathKeyCollision(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	ctx := context.Background()
	require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
	wanted, err := dto.ParseWorkspacePath("notes/a.md")
	require.NoError(t, err)
	sum := blake3.Sum256([]byte(wanted))

	require.NoError(t, env.UserDB(env.UID).Create(&model.WorkspacePath{
		WorkspaceID:  workspaceRepositoryWorkspaceID,
		PathKey:      hex.EncodeToString(sum[:]),
		Path:         "notes/b.md",
		PathRevision: 1,
		Kind:         string(dto.WorkspaceEntryFile),
	}).Error)

	err = env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		_, err := tx.Path(workspaceRepositoryWorkspaceID, wanted)
		return err
	})
	var collision *domain.WorkspacePathKeyCollisionError
	require.ErrorAs(t, err, &collision)
	require.Equal(t, workspaceRepositoryWorkspaceID, collision.WorkspaceID)
	require.Equal(t, string(wanted), collision.RequestedPath)
	require.Equal(t, "notes/b.md", collision.StoredPath)
}

func TestWorkspaceRepositoryRejectsRenameOntoExistingPathWithoutChangingEitherRow(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	ctx := context.Background()
	require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
	sourcePath := dto.WorkspacePath("notes/source.md")
	targetPath := dto.WorkspacePath("notes/target.md")

	require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		if err := tx.CreateWorkspace(workspaceRepositoryWorkspace(0)); err != nil {
			return err
		}
		if err := tx.SavePath(domain.WorkspacePathRecord{
			WorkspaceID: workspaceRepositoryWorkspaceID, Path: sourcePath,
			PathRevision: 1, Kind: dto.WorkspaceEntryFile, Size: 1,
		}); err != nil {
			return err
		}
		return tx.SavePath(domain.WorkspacePathRecord{
			WorkspaceID: workspaceRepositoryWorkspaceID, Path: targetPath,
			PathRevision: 2, Kind: dto.WorkspaceEntryFile, Size: 2,
		})
	}))

	var sourceBefore, targetBefore *domain.WorkspacePathRecord
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		var err error
		sourceBefore, err = tx.Path(workspaceRepositoryWorkspaceID, sourcePath)
		if err != nil {
			return err
		}
		targetBefore, err = tx.Path(workspaceRepositoryWorkspaceID, targetPath)
		return err
	}))

	rename := *sourceBefore
	rename.Path = targetPath
	rename.PathRevision = 3
	rename.Size = 3
	err := env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		return tx.SavePath(rename)
	})
	var unique *domain.WorkspaceUniqueConstraintError
	require.ErrorAs(t, err, &unique)

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		sourceAfter, err := tx.Path(workspaceRepositoryWorkspaceID, sourcePath)
		if err != nil {
			return err
		}
		targetAfter, err := tx.Path(workspaceRepositoryWorkspaceID, targetPath)
		if err != nil {
			return err
		}
		require.Equal(t, sourceBefore, sourceAfter)
		require.Equal(t, targetBefore, targetAfter)
		return nil
	}))
}

func TestWorkspaceRepositoryWriteTransactionRollsBackAllEightTables(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	ctx := context.Background()
	require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
	require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		return tx.CreateWorkspace(workspaceRepositoryWorkspace(7))
	}))

	hash := workspaceRepositoryHash("ab")
	path := dto.WorkspacePath("notes/a.md")
	conflictRevision, parseErr := dto.ParseWorkspaceConflictRevision("8")
	require.NoError(t, parseErr)
	rollbackErr := errors.New("roll back workspace transaction")
	err := env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		if err := tx.SaveWorkspace(workspaceRepositoryWorkspace(8)); err != nil {
			return err
		}
		if err := tx.SaveClient(domain.WorkspaceClientRecord{
			WorkspaceID: workspaceRepositoryWorkspaceID,
			ClientID:    workspaceRepositoryClientID,
			LastSeenAt:  time.Unix(100, 0).UTC(),
		}); err != nil {
			return err
		}
		if err := tx.SavePath(domain.WorkspacePathRecord{
			WorkspaceID: workspaceRepositoryWorkspaceID, Path: path, PathRevision: 8,
			Kind: dto.WorkspaceEntryFile, ContentHash: &hash, Size: 3, ModifiedAtMS: 1,
		}); err != nil {
			return err
		}
		if err := tx.SaveEvent(domain.WorkspaceEventRecord{
			WorkspaceID: workspaceRepositoryWorkspaceID, Revision: 8,
			OperationID: workspaceRepositoryOperationID, OriginClientID: workspaceRepositoryClientID,
			MutationJSON: []byte(`{"kind":"upsert_file"}`), PathStateJSON: []byte(`{"path":"notes/a.md"}`),
			CreatedAt: time.Unix(101, 0).UTC(),
		}); err != nil {
			return err
		}
		if err := tx.SaveOperation(domain.WorkspaceOperationRecord{
			WorkspaceID: workspaceRepositoryWorkspaceID, ClientID: workspaceRepositoryClientID,
			OperationID: workspaceRepositoryOperationID, RequestKind: "upsert_file",
			RequestDigest: strings.Repeat("c", 64), State: "terminal",
			CreatedAt: time.Unix(102, 0).UTC(), UpdatedAt: time.Unix(102, 0).UTC(),
		}); err != nil {
			return err
		}
		if err := tx.SaveConflict(domain.WorkspaceConflictRecord{
			WorkspaceID: workspaceRepositoryWorkspaceID, ConflictID: workspaceRepositoryConflictID,
			ConflictRevision: conflictRevision, Path: path, Kind: dto.WorkspaceConflictContent, Status: "open",
			AncestorJSON: []byte(`{}`), CurrentJSON: []byte(`{}`), IncomingJSON: []byte(`{}`),
			CreatedByOperationID: workspaceRepositoryOperationID,
			CreatedAt:            time.Unix(103, 0).UTC(), UpdatedAt: time.Unix(103, 0).UTC(),
		}); err != nil {
			return err
		}
		if err := tx.SaveBlob(domain.WorkspaceBlobRecord{
			ContentHash: hash, Size: 3, UTF8Valid: true,
			CreatedAt: time.Unix(104, 0).UTC(), UpdatedAt: time.Unix(104, 0).UTC(),
		}); err != nil {
			return err
		}
		if err := tx.AddBlobRef(domain.WorkspaceBlobRefRecord{
			ContentHash: hash, OwnerType: "path", OwnerKey: workspaceRepositoryWorkspaceID + ":notes/a.md",
		}, time.Unix(105, 0).UTC()); err != nil {
			return err
		}
		return rollbackErr
	})
	require.ErrorIs(t, err, rollbackErr)

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		workspace, err := tx.Workspace(workspaceRepositoryWorkspaceID)
		require.NoError(t, err)
		require.Equal(t, dto.WorkspaceRevision(7), workspace.GlobalRevision)

		_, err = tx.Client(workspaceRepositoryWorkspaceID, workspaceRepositoryClientID)
		require.ErrorIs(t, err, domain.ErrWorkspaceRecordNotFound)
		paths, err := tx.Paths(workspaceRepositoryWorkspaceID)
		require.NoError(t, err)
		require.Empty(t, paths)
		events, err := tx.EventsAfter(workspaceRepositoryWorkspaceID, 0, 8)
		require.NoError(t, err)
		require.Empty(t, events)
		_, err = tx.Operation(workspaceRepositoryClientID, workspaceRepositoryOperationID)
		require.ErrorIs(t, err, domain.ErrWorkspaceRecordNotFound)
		_, err = tx.Conflict(workspaceRepositoryWorkspaceID, workspaceRepositoryConflictID)
		require.ErrorIs(t, err, domain.ErrWorkspaceRecordNotFound)
		_, err = tx.Blob(hash)
		require.ErrorIs(t, err, domain.ErrWorkspaceRecordNotFound)
		return nil
	}))
	var refCount int64
	require.NoError(t, env.UserDB(env.UID).Model(&model.WorkspaceBlobRef{}).Count(&refCount).Error)
	require.Zero(t, refCount)
}

func TestWorkspaceRepositoryRefInsertDeleteKeepsCountAndUnreferencedAtAtomic(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	ctx := context.Background()
	require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
	hash := workspaceRepositoryHash("cd")
	ownerKey := workspaceRepositoryWorkspaceID + ":notes/a.md"
	addedAt := time.Unix(200, 0).UTC()
	removedAt := time.Unix(201, 0).UTC()

	require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		if err := tx.SaveBlob(domain.WorkspaceBlobRecord{ContentHash: hash, Size: 3, UTF8Valid: true}); err != nil {
			return err
		}
		ref := domain.WorkspaceBlobRefRecord{ContentHash: hash, OwnerType: "path", OwnerKey: ownerKey}
		if err := tx.AddBlobRef(ref, addedAt); err != nil {
			return err
		}
		return tx.AddBlobRef(ref, addedAt)
	}))
	workspaceRepositoryRequireBlobState(t, env, hash, 1, nil)
	workspaceRepositoryRequireRefCount(t, env.UserDB(env.UID), 1)

	require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		return tx.RemoveBlobRefs("path", ownerKey, removedAt)
	}))
	workspaceRepositoryRequireBlobState(t, env, hash, 0, &removedAt)
	workspaceRepositoryRequireRefCount(t, env.UserDB(env.UID), 0)

	require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		return tx.AddBlobRef(domain.WorkspaceBlobRefRecord{
			ContentHash: hash, OwnerType: "path", OwnerKey: ownerKey,
		}, addedAt)
	}))
	workspaceRepositoryRequireBlobState(t, env, hash, 1, nil)
	require.NoError(t, env.UserDB(env.UID).Model(&model.WorkspaceBlob{}).
		Where("content_hash = ?", hash).Update("ref_count", 0).Error)

	err := env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		return tx.RemoveBlobRefs("path", ownerKey, removedAt)
	})
	var underflow *domain.WorkspaceBlobRefUnderflowError
	require.ErrorAs(t, err, &underflow)
	require.Equal(t, hash, underflow.ContentHash)
	workspaceRepositoryRequireRefCount(t, env.UserDB(env.UID), 1)
}

func TestWorkspaceRepositoryAddBlobRefUsesAtomicIdempotentInsert(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	ctx := context.Background()
	require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
	hash := workspaceRepositoryHash("23")
	ref := domain.WorkspaceBlobRefRecord{
		ContentHash: hash,
		OwnerType:   "path",
		OwnerKey:    workspaceRepositoryWorkspaceID + ":atomic.md",
	}
	require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		return tx.SaveBlob(domain.WorkspaceBlobRecord{ContentHash: hash, Size: 3})
	}))

	userDB := env.UserDB(env.UID)
	const callbackName = "workspace_repository:capture_atomic_blob_ref_insert"
	var sawDoNothing atomic.Bool
	require.NoError(t, userDB.Callback().Create().Before("gorm:create").Register(callbackName, func(db *gorm.DB) {
		if db.Statement.Table != model.TableNameWorkspaceBlobRef {
			return
		}
		conflictClause, ok := db.Statement.Clauses["ON CONFLICT"]
		if !ok {
			return
		}
		onConflict, ok := conflictClause.Expression.(clause.OnConflict)
		if ok && onConflict.DoNothing {
			sawDoNothing.Store(true)
		}
	}))
	t.Cleanup(func() {
		require.NoError(t, userDB.Callback().Create().Remove(callbackName))
	})

	const writers = 8
	start := make(chan struct{})
	errs := make(chan error, writers)
	var wg sync.WaitGroup
	for i := 0; i < writers; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			<-start
			errs <- env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
				return tx.AddBlobRef(ref, time.Unix(220, 0).UTC())
			})
		}()
	}
	close(start)
	wg.Wait()
	close(errs)
	for err := range errs {
		require.NoError(t, err)
	}

	require.True(t, sawDoNothing.Load(), "blob-ref insert must use an atomic ON CONFLICT DO NOTHING statement")
	workspaceRepositoryRequireRefCount(t, userDB, 1)
	workspaceRepositoryRequireBlobState(t, env, hash, 1, nil)
}

func TestWorkspaceRepositoryAddBlobRefInsertsRefBeforeLockingBlob(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	ctx := context.Background()
	require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
	hash := workspaceRepositoryHash("24")
	require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		return tx.SaveBlob(domain.WorkspaceBlobRecord{ContentHash: hash, Size: 1})
	}))

	userDB := env.UserDB(env.UID)
	const createCallback = "workspace_repository:capture_add_ref_insert_order"
	const queryCallback = "workspace_repository:capture_add_blob_lock_order"
	var events []string
	var eventsMu sync.Mutex
	require.NoError(t, userDB.Callback().Create().Before("gorm:create").Register(createCallback, func(db *gorm.DB) {
		if db.Statement.Table == model.TableNameWorkspaceBlobRef {
			eventsMu.Lock()
			events = append(events, "ref_insert")
			eventsMu.Unlock()
		}
	}))
	require.NoError(t, userDB.Callback().Query().After("gorm:query").Register(queryCallback, func(db *gorm.DB) {
		if db.Statement.Table != model.TableNameWorkspaceBlob || len(db.Statement.Vars) != 1 {
			return
		}
		queriedHash, ok := db.Statement.Vars[0].(string)
		if !ok || queriedHash != string(hash) {
			return
		}
		eventsMu.Lock()
		events = append(events, "blob_lock")
		eventsMu.Unlock()
	}))
	t.Cleanup(func() {
		require.NoError(t, userDB.Callback().Create().Remove(createCallback))
		require.NoError(t, userDB.Callback().Query().Remove(queryCallback))
	})

	require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		return tx.AddBlobRef(domain.WorkspaceBlobRefRecord{
			ContentHash: hash, OwnerType: "path", OwnerKey: "lock-order",
		}, time.Unix(225, 0).UTC())
	}))
	require.Equal(t, []string{"ref_insert", "blob_lock"}, events)
}

func TestWorkspaceRepositoryConcurrentBlobRefAddRemovePreservesDerivedCount(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	ctx := context.Background()
	require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
	hash := workspaceRepositoryHash("25")
	require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		return tx.SaveBlob(domain.WorkspaceBlobRecord{ContentHash: hash, Size: 1})
	}))

	const owners = 32
	start := make(chan struct{})
	errs := make(chan error, owners*2)
	var wg sync.WaitGroup
	for i := 0; i < owners; i++ {
		ownerKey := fmt.Sprintf("add-remove-%02d", i)
		wg.Add(2)
		go func() {
			defer wg.Done()
			<-start
			errs <- env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
				return tx.AddBlobRef(domain.WorkspaceBlobRefRecord{
					ContentHash: hash, OwnerType: "event", OwnerKey: ownerKey,
				}, time.Unix(226, 0).UTC())
			})
		}()
		go func() {
			defer wg.Done()
			<-start
			errs <- env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
				return tx.RemoveBlobRefs("event", ownerKey, time.Unix(227, 0).UTC())
			})
		}()
	}
	close(start)
	wg.Wait()
	close(errs)
	for err := range errs {
		require.NoError(t, err)
	}

	var storedRefs int64
	require.NoError(t, env.UserDB(env.UID).Model(&model.WorkspaceBlobRef{}).
		Where("content_hash = ?", hash).Count(&storedRefs).Error)
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		blob, err := tx.Blob(hash)
		if err != nil {
			return err
		}
		require.Equal(t, storedRefs, blob.RefCount)
		if storedRefs == 0 {
			require.NotNil(t, blob.UnreferencedAt)
		} else {
			require.Nil(t, blob.UnreferencedAt)
		}
		return nil
	}))
}

func TestWorkspaceRepositoryAddBlobRefRollsBackInsertedRefWhenBlobIsMissing(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	ctx := context.Background()
	require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
	hash := workspaceRepositoryHash("26")
	err := env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		return tx.AddBlobRef(domain.WorkspaceBlobRefRecord{
			ContentHash: hash, OwnerType: "path", OwnerKey: "missing-blob",
		}, time.Unix(228, 0).UTC())
	})
	require.ErrorIs(t, err, domain.ErrWorkspaceRecordNotFound)
	workspaceRepositoryRequireRefCount(t, env.UserDB(env.UID), 0)
}

func TestWorkspaceRepositoryRemoveBlobRefsLocksHashesInSortedOrder(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	ctx := context.Background()
	require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
	ownerKey := workspaceRepositoryWorkspaceID + ":ordered-removal"
	pairs := []string{"0c", "03", "0a", "01", "08", "05", "0b", "02", "09", "06", "04", "07"}
	hashes := make([]string, 0, len(pairs))

	require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		for _, pair := range pairs {
			hash := workspaceRepositoryHash(pair)
			hashes = append(hashes, string(hash))
			if err := tx.SaveBlob(domain.WorkspaceBlobRecord{ContentHash: hash, Size: 1}); err != nil {
				return err
			}
			if err := tx.AddBlobRef(domain.WorkspaceBlobRefRecord{
				ContentHash: hash, OwnerType: "event", OwnerKey: ownerKey,
			}, time.Unix(230, 0).UTC()); err != nil {
				return err
			}
		}
		return nil
	}))

	userDB := env.UserDB(env.UID)
	const callbackName = "workspace_repository:capture_blob_lock_order"
	var locked []string
	var lockedMu sync.Mutex
	require.NoError(t, userDB.Callback().Query().After("gorm:query").Register(callbackName, func(db *gorm.DB) {
		if db.Statement.Table != model.TableNameWorkspaceBlob || len(db.Statement.Vars) != 1 {
			return
		}
		hash, ok := db.Statement.Vars[0].(string)
		if !ok || !strings.HasPrefix(hash, "blake3:") {
			return
		}
		lockedMu.Lock()
		locked = append(locked, hash)
		lockedMu.Unlock()
	}))
	t.Cleanup(func() {
		require.NoError(t, userDB.Callback().Query().Remove(callbackName))
	})

	require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		return tx.RemoveBlobRefs("event", ownerKey, time.Unix(231, 0).UTC())
	}))
	want := append([]string(nil), hashes...)
	sort.Strings(want)
	require.Equal(t, want, locked)
}

func TestWorkspaceRepositoryConcurrentBlobRefRemovalsRemainAtomic(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	ctx := context.Background()
	require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
	hashes := []dto.WorkspaceContentHash{
		workspaceRepositoryHash("31"),
		workspaceRepositoryHash("32"),
		workspaceRepositoryHash("33"),
		workspaceRepositoryHash("34"),
	}
	ownerKeys := []string{"concurrent-owner-a", "concurrent-owner-b"}

	require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		for _, hash := range hashes {
			if err := tx.SaveBlob(domain.WorkspaceBlobRecord{ContentHash: hash, Size: 1}); err != nil {
				return err
			}
			for _, ownerKey := range ownerKeys {
				if err := tx.AddBlobRef(domain.WorkspaceBlobRefRecord{
					ContentHash: hash, OwnerType: "conflict", OwnerKey: ownerKey,
				}, time.Unix(240, 0).UTC()); err != nil {
					return err
				}
			}
		}
		return nil
	}))

	start := make(chan struct{})
	errs := make(chan error, len(ownerKeys))
	var wg sync.WaitGroup
	for i, ownerKey := range ownerKeys {
		wg.Add(1)
		go func(ownerKey string, removedAt time.Time) {
			defer wg.Done()
			<-start
			errs <- env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
				return tx.RemoveBlobRefs("conflict", ownerKey, removedAt)
			})
		}(ownerKey, time.Unix(241+int64(i), 0).UTC())
	}
	close(start)
	wg.Wait()
	close(errs)
	for err := range errs {
		require.NoError(t, err)
	}

	workspaceRepositoryRequireRefCount(t, env.UserDB(env.UID), 0)
	for _, hash := range hashes {
		require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
			blob, err := tx.Blob(hash)
			if err != nil {
				return err
			}
			require.Zero(t, blob.RefCount)
			require.NotNil(t, blob.UnreferencedAt)
			return nil
		}))
	}
}

func TestWorkspaceRepositoryReturnsTypedConstraintErrors(t *testing.T) {
	t.Run("unique", func(t *testing.T) {
		env := testutil.NewWorkspaceEnv(t)
		ctx := context.Background()
		require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
		require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
			return tx.CreateWorkspace(workspaceRepositoryWorkspace(0))
		}))
		err := env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
			return tx.CreateWorkspace(workspaceRepositoryWorkspace(0))
		})
		var unique *domain.WorkspaceUniqueConstraintError
		require.ErrorAs(t, err, &unique)
	})

	t.Run("overflow", func(t *testing.T) {
		env := testutil.NewWorkspaceEnv(t)
		ctx := context.Background()
		require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
		hash := workspaceRepositoryHash("41")
		require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
			return tx.SaveBlob(domain.WorkspaceBlobRecord{ContentHash: hash, RefCount: math.MaxInt64})
		}))
		err := env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
			return tx.AddBlobRef(domain.WorkspaceBlobRefRecord{
				ContentHash: hash, OwnerType: "path", OwnerKey: "overflow",
			}, time.Unix(250, 0).UTC())
		})
		var overflow *domain.WorkspaceCounterOverflowError
		require.ErrorAs(t, err, &overflow)
		workspaceRepositoryRequireRefCount(t, env.UserDB(env.UID), 0)
	})

	t.Run("underflow", func(t *testing.T) {
		env := testutil.NewWorkspaceEnv(t)
		ctx := context.Background()
		require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
		err := env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
			return tx.SaveBlob(domain.WorkspaceBlobRecord{
				ContentHash: workspaceRepositoryHash("42"), RefCount: -1,
			})
		})
		var underflow *domain.WorkspaceCounterUnderflowError
		require.ErrorAs(t, err, &underflow)
	})
}

func TestWorkspaceRepositoryReconcilesDerivedRefCount(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	ctx := context.Background()
	require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
	referencedHash := workspaceRepositoryHash("ef")
	unreferencedHash := workspaceRepositoryHash("01")
	staleUnreferencedAt := time.Unix(299, 0).UTC()
	reconciledAt := time.Unix(300, 0).UTC()

	require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		if err := tx.SaveBlob(domain.WorkspaceBlobRecord{
			ContentHash: referencedHash, Size: 3, RefCount: 99, UnreferencedAt: &staleUnreferencedAt,
		}); err != nil {
			return err
		}
		return tx.SaveBlob(domain.WorkspaceBlobRecord{ContentHash: unreferencedHash, Size: 4, RefCount: 7})
	}))
	require.NoError(t, env.UserDB(env.UID).Create([]model.WorkspaceBlobRef{
		{ContentHash: string(referencedHash), OwnerType: "path", OwnerKey: "one"},
		{ContentHash: string(referencedHash), OwnerType: "event", OwnerKey: "two"},
	}).Error)

	require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		return tx.ReconcileBlobRefCounts(reconciledAt)
	}))
	workspaceRepositoryRequireBlobState(t, env, referencedHash, 2, nil)
	workspaceRepositoryRequireBlobState(t, env, unreferencedHash, 0, &reconciledAt)
}

func TestWorkspaceRepositoryReconcileBlobRefCountsUsesSetBasedSQL(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	ctx := context.Background()
	require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
	referencedHash := workspaceRepositoryHash("61")
	unreferencedHash := workspaceRepositoryHash("62")
	require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		if err := tx.SaveBlob(domain.WorkspaceBlobRecord{ContentHash: referencedHash, RefCount: 19}); err != nil {
			return err
		}
		return tx.SaveBlob(domain.WorkspaceBlobRecord{ContentHash: unreferencedHash, RefCount: 23})
	}))
	require.NoError(t, env.UserDB(env.UID).Create(&model.WorkspaceBlobRef{
		ContentHash: string(referencedHash), OwnerType: "path", OwnerKey: "set-based",
	}).Error)

	userDB := env.UserDB(env.UID)
	const callbackName = "workspace_repository:reject_materialized_blob_reconciliation"
	var materializingQueries atomic.Int64
	require.NoError(t, userDB.Callback().Query().Before("gorm:query").Register(callbackName, func(db *gorm.DB) {
		if db.Statement.Table == model.TableNameWorkspaceBlob || db.Statement.Table == model.TableNameWorkspaceBlobRef {
			materializingQueries.Add(1)
		}
	}))
	t.Cleanup(func() { require.NoError(t, userDB.Callback().Query().Remove(callbackName)) })

	reconciledAt := time.Unix(320, 0).UTC()
	require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		return tx.ReconcileBlobRefCounts(reconciledAt)
	}))
	require.Zero(t, materializingQueries.Load(), "reconciliation must not Find/Scan blob or ref tables into Go")
	workspaceRepositoryRequireBlobState(t, env, referencedHash, 1, nil)
	workspaceRepositoryRequireBlobState(t, env, unreferencedHash, 0, &reconciledAt)
}

func TestWorkspaceRepositoryBlobsAfterPaginatesCanonicalHashOrder(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	ctx := context.Background()
	require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
	hashes := []dto.WorkspaceContentHash{
		workspaceRepositoryHash("ef"),
		workspaceRepositoryHash("01"),
		workspaceRepositoryHash("7a"),
	}
	require.NoError(t, env.WorkspaceRepo.Write(ctx, env.UID, func(tx domain.WorkspaceWriteTx) error {
		for i, hash := range hashes {
			if err := tx.SaveBlob(domain.WorkspaceBlobRecord{
				ContentHash: hash,
				Size:        uint64(i + 1),
				UTF8Valid:   i%2 == 0,
			}); err != nil {
				return err
			}
		}
		return nil
	}))

	var firstPage, secondPage []domain.WorkspaceBlobRecord
	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		var err error
		firstPage, err = tx.BlobsAfter(nil, 2)
		if err != nil {
			return err
		}
		cursor := firstPage[len(firstPage)-1].ContentHash
		secondPage, err = tx.BlobsAfter(&cursor, 2)
		return err
	}))
	require.Equal(t, []dto.WorkspaceContentHash{hashes[1], hashes[2]}, []dto.WorkspaceContentHash{
		firstPage[0].ContentHash,
		firstPage[1].ContentHash,
	})
	require.Equal(t, []dto.WorkspaceContentHash{hashes[0]}, []dto.WorkspaceContentHash{
		secondPage[0].ContentHash,
	})

	userDB := env.UserDB(env.UID)
	const callbackName = "workspace_repository:reject_unbounded_blob_scan"
	var blobQueries atomic.Int64
	require.NoError(t, userDB.Callback().Query().Before("gorm:query").Register(callbackName, func(db *gorm.DB) {
		if db.Statement.Table == model.TableNameWorkspaceBlob {
			blobQueries.Add(1)
		}
	}))
	t.Cleanup(func() { require.NoError(t, userDB.Callback().Query().Remove(callbackName)) })

	require.NoError(t, env.WorkspaceRepo.Read(ctx, env.UID, func(tx domain.WorkspaceReadTx) error {
		for _, limit := range []int{0, -1} {
			_, err := tx.BlobsAfter(nil, limit)
			require.Error(t, err)
		}
		invalidCursor := dto.WorkspaceContentHash("blake3:" + strings.Repeat("A0", 32))
		_, err := tx.BlobsAfter(&invalidCursor, 1)
		require.Error(t, err)
		return nil
	}))
	require.Zero(t, blobQueries.Load())
}

func workspaceRepositoryWorkspace(revision dto.WorkspaceRevision) domain.WorkspaceRecord {
	return domain.WorkspaceRecord{
		WorkspaceID: workspaceRepositoryWorkspaceID, GlobalRevision: revision,
		CreatedAt: time.Unix(99, 0).UTC(), UpdatedAt: time.Unix(99, 0).UTC(),
	}
}

func workspaceRepositoryHash(pair string) dto.WorkspaceContentHash {
	return dto.WorkspaceContentHash("blake3:" + strings.Repeat(pair, 32))
}

func workspaceRepositoryColumn(databaseType string, nullable bool) workspaceRepositoryColumnSnapshot {
	return workspaceRepositoryColumnSnapshot{
		DatabaseType: databaseType,
		DeclaredType: databaseType,
		Nullable:     nullable,
	}
}

func workspaceRepositorySizedColumn(databaseType string, length int64, nullable bool) workspaceRepositoryColumnSnapshot {
	return workspaceRepositoryColumnSnapshot{
		DatabaseType: databaseType,
		DeclaredType: fmt.Sprintf("%s(%d)", databaseType, length),
		Length:       length,
		Nullable:     nullable,
	}
}

func workspaceRepositoryTables(t *testing.T, db *gorm.DB) []string {
	t.Helper()
	tables, err := db.Migrator().GetTables()
	require.NoError(t, err)
	portableTables := make([]string, 0, len(tables))
	for _, table := range tables {
		if !strings.HasPrefix(table, "sqlite_") {
			portableTables = append(portableTables, table)
		}
	}
	sort.Strings(portableTables)
	return portableTables
}

func workspaceRepositorySchema(t *testing.T, db *gorm.DB) map[string]workspaceRepositorySchemaSnapshot {
	t.Helper()
	schema := make(map[string]workspaceRepositorySchemaSnapshot, len(workspaceRepositoryTableNames))
	for _, table := range workspaceRepositoryTableNames {
		columns, err := db.Migrator().ColumnTypes(table)
		require.NoError(t, err)
		indexes, err := db.Migrator().GetIndexes(table)
		require.NoError(t, err)
		snapshot := workspaceRepositorySchemaSnapshot{
			Columns: make(map[string]workspaceRepositoryColumnSnapshot, len(columns)),
		}
		for _, column := range columns {
			declaredType, ok := column.ColumnType()
			require.Truef(t, ok, "column %s.%s has no declared type", table, column.Name())
			nullable, ok := column.Nullable()
			require.Truef(t, ok, "column %s.%s has no nullability", table, column.Name())
			length, hasLength := column.Length()
			if !hasLength {
				length = 0
			}
			snapshot.Columns[column.Name()] = workspaceRepositoryColumnSnapshot{
				DatabaseType: strings.ToLower(column.DatabaseTypeName()),
				DeclaredType: strings.ToLower(declaredType),
				Length:       length,
				Nullable:     nullable,
			}
			if primary, ok := column.PrimaryKey(); ok && primary {
				snapshot.PrimaryKey = append(snapshot.PrimaryKey, column.Name())
			}
		}
		for _, index := range indexes {
			if !strings.HasPrefix(index.Name(), "idx_workspace_") {
				continue
			}
			unique, _ := index.Unique()
			snapshot.Indexes = append(snapshot.Indexes, fmt.Sprintf(
				"%s|%t|%s",
				index.Name(),
				unique,
				strings.Join(index.Columns(), ","),
			))
		}
		sort.Strings(snapshot.PrimaryKey)
		sort.Strings(snapshot.Indexes)
		schema[table] = snapshot
	}
	return schema
}

func workspaceRepositoryRequireCheckConstraint(
	t *testing.T,
	db *gorm.DB,
	modelValue any,
	name, expression string,
) {
	t.Helper()
	require.True(t, db.Migrator().HasConstraint(modelValue, name))
	stmt := &gorm.Statement{DB: db}
	require.NoError(t, stmt.Parse(modelValue))
	var ddl string
	require.NoError(t, db.Raw(
		"SELECT sql FROM sqlite_master WHERE type = 'table' AND name = ?",
		stmt.Schema.Table,
	).Scan(&ddl).Error)
	normalized := strings.ToLower(strings.NewReplacer("`", "", `"`, "").Replace(ddl))
	normalized = strings.Join(strings.Fields(normalized), " ")
	require.Contains(t, normalized, expression)
}

func workspaceRepositoryRequireBlobState(
	t *testing.T,
	env *testutil.WorkspaceEnv,
	hash dto.WorkspaceContentHash,
	wantRefCount int64,
	wantUnreferencedAt *time.Time,
) {
	t.Helper()
	require.NoError(t, env.WorkspaceRepo.Read(context.Background(), env.UID, func(tx domain.WorkspaceReadTx) error {
		blob, err := tx.Blob(hash)
		if err != nil {
			return err
		}
		require.Equal(t, wantRefCount, blob.RefCount)
		if wantUnreferencedAt == nil {
			require.Nil(t, blob.UnreferencedAt)
		} else {
			require.NotNil(t, blob.UnreferencedAt)
			require.True(t, blob.UnreferencedAt.Equal(*wantUnreferencedAt))
		}
		return nil
	}))
}

func workspaceRepositoryRequireRefCount(t *testing.T, db *gorm.DB, want int64) {
	t.Helper()
	var count int64
	require.NoError(t, db.Model(&model.WorkspaceBlobRef{}).Count(&count).Error)
	require.Equal(t, want, count)
}
