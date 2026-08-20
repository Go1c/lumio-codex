package service

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/haierkeys/fast-note-sync-service/internal/config"
	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/internal/model"
	"github.com/haierkeys/fast-note-sync-service/internal/testutil"
	"github.com/stretchr/testify/require"
	"github.com/zeebo/blake3"
)

func TestWorkspaceBlobStorePutStreamsVerifiesSizeHashAndDeduplicates(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	store := NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot))
	content := []byte{0xff, 0xfe, 0x00, 0x80, 'b', 'l', 'o', 'b'}
	hash := workspaceBlobStoreHash(content)
	reader := &workspaceBlobStoreMultiReader{data: content, chunkSize: 2}

	require.NoError(t, store.Put(context.Background(), env.UID, hash, uint64(len(content)), reader))
	require.Greater(t, reader.reads, 1)
	require.Equal(t, content, workspaceBlobStoreReadFile(t, workspaceBlobStoreFinalPath(env.BlobRoot, env.UID, hash)))
	record := workspaceBlobStoreRequireBlob(t, env, env.UID, hash)
	require.Equal(t, uint64(len(content)), record.Size)
	require.False(t, record.UTF8Valid)
	require.Zero(t, record.RefCount)
	require.NotNil(t, record.UnreferencedAt)
	textContent := []byte("valid UTF-8 世界 text\n")
	textHash := workspaceBlobStoreHash(textContent)
	textReader := &workspaceBlobStoreMultiReader{data: textContent, chunkSize: 1}
	require.NoError(t, store.Put(
		context.Background(),
		env.UID,
		textHash,
		uint64(len(textContent)),
		textReader,
	))
	require.Greater(t, textReader.reads, len([]rune(string(textContent))))
	require.True(t, workspaceBlobStoreRequireBlob(t, env, env.UID, textHash).UTF8Valid)

	const concurrentPuts = 8
	start := make(chan struct{})
	errs := make(chan error, concurrentPuts)
	var wg sync.WaitGroup
	for range concurrentPuts {
		wg.Add(1)
		go func() {
			defer wg.Done()
			<-start
			errs <- store.Put(context.Background(), env.UID, hash, uint64(len(content)), bytes.NewReader(content))
		}()
	}
	close(start)
	wg.Wait()
	close(errs)
	for err := range errs {
		require.NoError(t, err)
	}

	var count int64
	require.NoError(t, env.UserDB(env.UID).Model(&model.WorkspaceBlob{}).
		Where("content_hash = ?", string(hash)).Count(&count).Error)
	require.Equal(t, int64(1), count)
	require.Empty(t, workspaceBlobStoreReadDir(t, workspaceBlobStoreStageDir(env.BlobRoot, env.UID)))
	require.Equal(t, content, workspaceBlobStoreReadFile(t, workspaceBlobStoreFinalPath(env.BlobRoot, env.UID, hash)))
}

func TestWorkspaceBlobStoreHasChecksFinalizedRowAndExactSize(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	store := NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot))
	ctx := context.Background()
	content := []byte("finalized workspace blob")
	hash := workspaceBlobStoreHash(content)

	has, err := store.Has(ctx, env.UID, hash, uint64(len(content)))
	require.NoError(t, err)
	require.False(t, has)

	_, err = store.Has(ctx, 0, hash, uint64(len(content)))
	require.ErrorIs(t, err, domain.ErrWorkspaceInvalidUID)

	canceled, cancel := context.WithCancel(ctx)
	cancel()
	_, err = store.Has(canceled, env.UID, hash, uint64(len(content)))
	require.ErrorIs(t, err, context.Canceled)
	_, err = store.Has(ctx, env.UID, dto.WorkspaceContentHash("not-a-hash"), uint64(len(content)))
	require.Error(t, err)
	_, err = store.Has(ctx, env.UID, hash, dto.WorkspaceMaxBlobBytes+1)
	require.Error(t, err)

	require.NoError(t, store.Put(ctx, env.UID, hash, uint64(len(content)), bytes.NewReader(content)))
	has, err = store.Has(ctx, env.UID, hash, uint64(len(content)))
	require.NoError(t, err)
	require.True(t, has)
	has, err = store.Has(ctx, env.OtherUID, hash, uint64(len(content)))
	require.NoError(t, err)
	require.False(t, has)
	has, err = store.Has(ctx, env.UID, hash, uint64(len(content))+1)
	require.NoError(t, err)
	require.False(t, has)

	emptyHash := workspaceBlobStoreHash(nil)
	require.NoError(t, store.Put(ctx, env.UID, emptyHash, 0, bytes.NewReader(nil)))
	has, err = store.Has(ctx, env.UID, emptyHash, 0)
	require.NoError(t, err)
	require.True(t, has)
}

func TestWorkspaceBlobStoreOpenReturnsPersistedVerifiedSize(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	store := NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot))
	content := []byte("open returns its persisted size")
	hash := workspaceBlobStoreHash(content)
	require.NoError(t, store.Put(context.Background(), env.UID, hash, uint64(len(content)), bytes.NewReader(content)))

	opened, size, err := store.Open(context.Background(), env.UID, hash)
	require.NoError(t, err)
	require.Equal(t, uint64(len(content)), size)
	t.Cleanup(func() { require.NoError(t, opened.Close()) })
	got, err := io.ReadAll(opened)
	require.NoError(t, err)
	require.Equal(t, content, got)
}

func TestWorkspaceBlobStoreAcceptsZeroByteBlobWithoutBinaryFrameAssumption(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	store := NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot))
	emptyHash := dto.WorkspaceContentHash("blake3:af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262")

	require.NoError(t, store.Put(context.Background(), env.UID, emptyHash, 0, bytes.NewReader(nil)))
	opened, size, err := store.Open(context.Background(), env.UID, emptyHash)
	require.NoError(t, err)
	require.Zero(t, size)
	t.Cleanup(func() { require.NoError(t, opened.Close()) })
	got, err := io.ReadAll(opened)
	require.NoError(t, err)
	require.Empty(t, got)
	require.Zero(t, workspaceBlobStoreRequireBlob(t, env, env.UID, emptyHash).Size)
}

func TestWorkspaceBlobStorePartitionsSameHashByUID(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	store := NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot))
	content := []byte("same bytes in isolated user stores")
	hash := workspaceBlobStoreHash(content)

	require.NoError(t, store.Put(context.Background(), env.UID, hash, uint64(len(content)), bytes.NewReader(content)))
	require.NoError(t, store.Put(context.Background(), env.OtherUID, hash, uint64(len(content)), bytes.NewReader(content)))
	firstPath := workspaceBlobStoreFinalPath(env.BlobRoot, env.UID, hash)
	secondPath := workspaceBlobStoreFinalPath(env.BlobRoot, env.OtherUID, hash)
	require.NotEqual(t, firstPath, secondPath)
	require.Contains(t, firstPath, string(filepath.Separator)+"user_41"+string(filepath.Separator))
	require.Contains(t, secondPath, string(filepath.Separator)+"user_42"+string(filepath.Separator))

	now := time.Now().UTC().Truncate(time.Second)
	workspaceBlobStoreExpireBlob(t, env, env.UID, hash, now.Add(-2*time.Hour))
	require.NoError(t, os.Chtimes(firstPath, now.Add(-2*time.Hour), now.Add(-2*time.Hour)))
	require.NoError(t, store.ReconcileAndGC(context.Background(), env.UID, now))
	require.NoFileExists(t, firstPath)
	require.FileExists(t, secondPath)
	workspaceBlobStoreRequireNoBlob(t, env, env.UID, hash)
	workspaceBlobStoreRequireBlob(t, env, env.OtherUID, hash)
}

func TestWorkspaceBlobStoreFailurePublishesNeitherFinalNorRow(t *testing.T) {
	cases := []struct {
		name        string
		content     []byte
		declared    func([]byte) dto.WorkspaceContentHash
		size        func([]byte) uint64
		reader      func([]byte) io.Reader
		canonicalDB bool
	}{
		{
			name: "reader failure", content: []byte("known-prefix-and-tail"),
			declared: workspaceBlobStoreHash, size: func(data []byte) uint64 { return uint64(len(data)) },
			reader: func(data []byte) io.Reader { return &workspaceBlobStoreFailReader{data: data[:6]} }, canonicalDB: true,
		},
		{
			name: "short size", content: []byte("short"),
			declared: workspaceBlobStoreHash, size: func(data []byte) uint64 { return uint64(len(data) + 1) },
			reader: func(data []byte) io.Reader { return bytes.NewReader(data) }, canonicalDB: true,
		},
		{
			name: "extra byte", content: []byte("extra"),
			declared: workspaceBlobStoreHash, size: func(data []byte) uint64 { return uint64(len(data) - 1) },
			reader: func(data []byte) io.Reader { return bytes.NewReader(data) }, canonicalDB: true,
		},
		{
			name: "hash mismatch", content: []byte("wrong digest"),
			declared: func([]byte) dto.WorkspaceContentHash { return workspaceBlobStoreHash([]byte("different")) },
			size:     func(data []byte) uint64 { return uint64(len(data)) },
			reader:   func(data []byte) io.Reader { return bytes.NewReader(data) }, canonicalDB: true,
		},
		{
			name: "uppercase hash", content: []byte("upper"),
			declared: func(data []byte) dto.WorkspaceContentHash {
				hash := string(workspaceBlobStoreHash(data))
				return dto.WorkspaceContentHash("blake3:" + strings.ToUpper(strings.TrimPrefix(hash, "blake3:")))
			},
			size:   func(data []byte) uint64 { return uint64(len(data)) },
			reader: func(data []byte) io.Reader { return bytes.NewReader(data) }, canonicalDB: false,
		},
		{
			name: "oversize", content: []byte("small reader"),
			declared: workspaceBlobStoreHash, size: func([]byte) uint64 { return dto.WorkspaceMaxBlobBytes + 1 },
			reader: func(data []byte) io.Reader { return bytes.NewReader(data) }, canonicalDB: true,
		},
	}

	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			env := testutil.NewWorkspaceEnv(t)
			store := NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot))
			hash := tc.declared(tc.content)
			if tc.canonicalDB {
				require.NoError(t, env.WorkspaceRepo.Migrate(context.Background(), env.UID))
			}
			require.Error(t, store.Put(context.Background(), env.UID, hash, tc.size(tc.content), tc.reader(tc.content)))
			if tc.canonicalDB {
				require.NoFileExists(t, workspaceBlobStoreFinalPath(env.BlobRoot, env.UID, hash))
				workspaceBlobStoreRequireNoBlob(t, env, env.UID, hash)
			}
			require.Empty(t, workspaceBlobStoreStageEntriesIfPresent(t, env.BlobRoot, env.UID))
		})
	}

	t.Run("invalid uid", func(t *testing.T) {
		env := testutil.NewWorkspaceEnv(t)
		store := NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot))
		content := []byte("uid")
		hash := workspaceBlobStoreHash(content)
		for _, uid := range []int64{0, -1} {
			require.ErrorIs(t, store.Put(context.Background(), uid, hash, uint64(len(content)), bytes.NewReader(content)), domain.ErrWorkspaceInvalidUID)
		}
		require.NoDirExists(t, filepath.Join(env.BlobRoot, "user_0"))
		require.NoDirExists(t, filepath.Join(env.BlobRoot, "user_-1"))
	})
}

func TestWorkspaceBlobStorePublishesAfterFileSyncRenameAndDirectorySync(t *testing.T) {
	injected := errors.New("injected durability failure")
	cases := []struct {
		name           string
		failStageSync  error
		failStageClose error
		failRename     error
		failDirSync    error
		failRowWrite   error
		wantFinal      bool
		wantEvents     []string
	}{
		{name: "success", wantFinal: true, wantEvents: []string{"create_temp", "stage_sync", "stage_close", "rename", "sync_fanout", "save_blob"}},
		{name: "stage sync", failStageSync: injected, wantEvents: []string{"create_temp", "stage_sync", "stage_close"}},
		{name: "stage close", failStageClose: injected, wantEvents: []string{"create_temp", "stage_sync", "stage_close"}},
		{name: "rename", failRename: injected, wantEvents: []string{"create_temp", "stage_sync", "stage_close", "rename"}},
		{name: "directory sync", failDirSync: injected, wantFinal: true, wantEvents: []string{"create_temp", "stage_sync", "stage_close", "rename", "sync_fanout"}},
		{name: "row write", failRowWrite: injected, wantFinal: true, wantEvents: []string{"create_temp", "stage_sync", "stage_close", "rename", "sync_fanout", "save_blob"}},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			env := testutil.NewWorkspaceEnv(t)
			require.NoError(t, env.WorkspaceRepo.Migrate(context.Background(), env.UID))
			events := &workspaceBlobStoreEvents{}
			repo := &workspaceBlobStoreRecordingRepository{
				WorkspaceRepository: env.WorkspaceRepo,
				events:              events,
				failSaveBlob:        tc.failRowWrite,
			}
			filesystem := &workspaceBlobStoreRecordingFS{
				events: events, failStageSync: tc.failStageSync, failStageClose: tc.failStageClose,
				failRename: tc.failRename, failDirSync: tc.failDirSync,
			}
			store := newWorkspaceBlobStore(repo, workspaceBlobStoreConfig(t, env.BlobRoot), filesystem)
			content := []byte("durable publication " + tc.name)
			hash := workspaceBlobStoreHash(content)
			finalPath := workspaceBlobStoreFinalPath(env.BlobRoot, env.UID, hash)

			err := store.Put(context.Background(), env.UID, hash, uint64(len(content)), bytes.NewReader(content))
			if tc.name == "success" {
				require.NoError(t, err)
				workspaceBlobStoreRequireBlob(t, env, env.UID, hash)
			} else {
				require.ErrorIs(t, err, injected)
				workspaceBlobStoreRequireNoBlob(t, env, env.UID, hash)
			}
			require.Equal(t, tc.wantEvents, events.snapshot())
			require.NotNil(t, filesystem.created)
			require.True(t, filesystem.created.closed.Load())
			if tc.failRename != nil || tc.name == "success" || tc.failDirSync != nil || tc.failRowWrite != nil {
				require.True(t, filesystem.renameSawClosed.Load(), "stage must be closed before rename")
			}
			require.Empty(t, workspaceBlobStoreStageEntriesIfPresent(t, env.BlobRoot, env.UID))

			if tc.wantFinal {
				require.Equal(t, content, workspaceBlobStoreReadFile(t, finalPath))
			} else {
				require.NoFileExists(t, finalPath)
			}
			if tc.name != "success" && tc.wantFinal {
				now := time.Now().UTC().Truncate(time.Second)
				require.NoError(t, os.Chtimes(finalPath, now.Add(-2*time.Hour), now.Add(-2*time.Hour)))
				plainStore := NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot))
				require.NoError(t, plainStore.ReconcileAndGC(context.Background(), env.UID, now))
				require.NoFileExists(t, finalPath)
			}
		})
	}
}

func TestWorkspaceBlobStoreRetrySyncsSurvivingFinalBeforePublishingRow(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	ctx := context.Background()
	require.NoError(t, env.WorkspaceRepo.Migrate(ctx, env.UID))
	content := []byte("surviving final needs a retry durability barrier")
	hash := workspaceBlobStoreHash(content)
	finalPath := workspaceBlobStoreFinalPath(env.BlobRoot, env.UID, hash)
	injected := errors.New("injected first parent sync failure")

	firstEvents := &workspaceBlobStoreEvents{}
	firstStore := newWorkspaceBlobStore(
		&workspaceBlobStoreRecordingRepository{WorkspaceRepository: env.WorkspaceRepo, events: firstEvents},
		workspaceBlobStoreConfig(t, env.BlobRoot),
		&workspaceBlobStoreRecordingFS{events: firstEvents, failDirSync: injected},
	)
	err := firstStore.Put(ctx, env.UID, hash, uint64(len(content)), bytes.NewReader(content))
	require.ErrorIs(t, err, injected)
	require.Equal(t, content, workspaceBlobStoreReadFile(t, finalPath))
	workspaceBlobStoreRequireNoBlob(t, env, env.UID, hash)
	require.Empty(t, workspaceBlobStoreStageEntriesIfPresent(t, env.BlobRoot, env.UID))

	secondEvents := &workspaceBlobStoreEvents{}
	secondStore := newWorkspaceBlobStore(
		&workspaceBlobStoreRecordingRepository{WorkspaceRepository: env.WorkspaceRepo, events: secondEvents},
		workspaceBlobStoreConfig(t, env.BlobRoot),
		&workspaceBlobStoreRecordingFS{events: secondEvents},
	)
	require.NoError(t, secondStore.Put(ctx, env.UID, hash, uint64(len(content)), bytes.NewReader(content)))
	require.Equal(t, []string{
		"create_temp", "stage_sync", "stage_close", "sync_fanout", "save_blob",
	}, secondEvents.snapshot())
	workspaceBlobStoreRequireBlob(t, env, env.UID, hash)
	require.Equal(t, content, workspaceBlobStoreReadFile(t, finalPath))
	require.Empty(t, workspaceBlobStoreStageEntriesIfPresent(t, env.BlobRoot, env.UID))
}

func TestWorkspaceBlobStoreOpenRejectsMissingOrCorruptFinal(t *testing.T) {
	for _, tc := range []struct {
		name   string
		mutate func(t *testing.T, path string)
	}{
		{name: "missing", mutate: func(t *testing.T, path string) { require.NoError(t, os.Remove(path)) }},
		{name: "corrupt size", mutate: func(t *testing.T, path string) { require.NoError(t, os.WriteFile(path, []byte("x"), 0600)) }},
		{name: "corrupt hash", mutate: func(t *testing.T, path string) {
			require.NoError(t, os.WriteFile(path, []byte("same length but bad"), 0600))
		}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			env := testutil.NewWorkspaceEnv(t)
			store := NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot))
			content := []byte("same length content")
			hash := workspaceBlobStoreHash(content)
			require.NoError(t, store.Put(context.Background(), env.UID, hash, uint64(len(content)), bytes.NewReader(content)))
			tc.mutate(t, workspaceBlobStoreFinalPath(env.BlobRoot, env.UID, hash))

			opened, size, err := store.Open(context.Background(), env.UID, hash)
			require.Error(t, err)
			require.Nil(t, opened)
			require.Zero(t, size)
			workspaceBlobStoreRequireBlob(t, env, env.UID, hash)
		})
	}
}

func TestWorkspaceBlobStoreOpenRejectsLstatToOpenSwapBeforeRead(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	cfg := workspaceBlobStoreConfig(t, env.BlobRoot)
	content := []byte("same bytes, different inode")
	hash := workspaceBlobStoreHash(content)
	plainStore := NewWorkspaceBlobStore(env.WorkspaceRepo, cfg)
	require.NoError(t, plainStore.Put(context.Background(), env.UID, hash, uint64(len(content)), bytes.NewReader(content)))
	finalPath := workspaceBlobStoreFinalPath(env.BlobRoot, env.UID, hash)
	outsidePath := filepath.Join(t.TempDir(), "outside")
	require.NoError(t, os.WriteFile(outsidePath, content, 0600))

	filesystem := &workspaceBlobStoreRecordingFS{events: &workspaceBlobStoreEvents{}}
	filesystem.openOverride = func(name string) (workspaceBlobFile, error) {
		if name != finalPath {
			return os.Open(name)
		}
		outside, err := os.Open(outsidePath)
		if err != nil {
			return nil, err
		}
		wrapped := &workspaceBlobStoreRecordingFile{workspaceBlobFile: outside}
		filesystem.opened = wrapped
		return wrapped, nil
	}
	store := newWorkspaceBlobStore(env.WorkspaceRepo, cfg, filesystem)

	opened, size, err := store.Open(context.Background(), env.UID, hash)
	require.Error(t, err)
	require.Nil(t, opened)
	require.Zero(t, size)
	require.NotNil(t, filesystem.opened)
	require.Zero(t, filesystem.opened.reads.Load(), "swapped handle must be rejected before reading")
	require.True(t, filesystem.opened.closed.Load(), "swapped handle must be closed")
	require.Equal(t, content, workspaceBlobStoreReadFile(t, outsidePath))
}

func TestWorkspaceBlobStoreReconcileRepairsRefCountAndCrashWindows(t *testing.T) {
	t.Run("repairs refcount and orphan windows", func(t *testing.T) {
		env := testutil.NewWorkspaceEnv(t)
		store := NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot))
		now := time.Now().UTC().Truncate(time.Second)

		referencedContent := []byte("referenced")
		referencedHash := workspaceBlobStoreHash(referencedContent)
		require.NoError(t, store.Put(context.Background(), env.UID, referencedHash, uint64(len(referencedContent)), bytes.NewReader(referencedContent)))
		workspaceBlobStoreAddRef(t, env, env.UID, referencedHash, "reconcile-ref", now.Add(-3*time.Hour))
		require.NoError(t, env.UserDB(env.UID).Model(&model.WorkspaceBlob{}).
			Where("content_hash = ?", string(referencedHash)).
			Updates(map[string]any{"ref_count": 99, "unreferenced_at": now.Add(-3 * time.Hour)}).Error)

		orphanContent := []byte("post-rename-pre-row")
		orphanHash := workspaceBlobStoreHash(orphanContent)
		orphanPath := workspaceBlobStoreWriteFinal(t, env.BlobRoot, env.UID, orphanHash, orphanContent)
		require.NoError(t, os.Chtimes(orphanPath, now.Add(-2*time.Hour), now.Add(-2*time.Hour)))

		missingContent := []byte("post-unlink-pre-row-delete")
		missingHash := workspaceBlobStoreHash(missingContent)
		require.NoError(t, store.Put(context.Background(), env.UID, missingHash, uint64(len(missingContent)), bytes.NewReader(missingContent)))
		workspaceBlobStoreExpireBlob(t, env, env.UID, missingHash, now.Add(-2*time.Hour))
		require.NoError(t, os.Remove(workspaceBlobStoreFinalPath(env.BlobRoot, env.UID, missingHash)))

		require.NoError(t, store.ReconcileAndGC(context.Background(), env.UID, now))
		repaired := workspaceBlobStoreRequireBlob(t, env, env.UID, referencedHash)
		require.Equal(t, int64(1), repaired.RefCount)
		require.Nil(t, repaired.UnreferencedAt)
		require.NoFileExists(t, orphanPath)
		workspaceBlobStoreRequireNoBlob(t, env, env.UID, orphanHash)
		workspaceBlobStoreRequireNoBlob(t, env, env.UID, missingHash)
	})

	for _, tc := range []struct {
		name   string
		mutate func(t *testing.T, path string)
	}{
		{name: "referenced missing", mutate: func(t *testing.T, path string) { require.NoError(t, os.Remove(path)) }},
		{name: "referenced corrupt", mutate: func(t *testing.T, path string) { require.NoError(t, os.WriteFile(path, []byte("corrupt"), 0600)) }},
	} {
		t.Run(tc.name, func(t *testing.T) {
			env := testutil.NewWorkspaceEnv(t)
			store := NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot))
			now := time.Now().UTC().Truncate(time.Second)
			content := []byte("must remain referenced")
			hash := workspaceBlobStoreHash(content)
			require.NoError(t, store.Put(context.Background(), env.UID, hash, uint64(len(content)), bytes.NewReader(content)))
			workspaceBlobStoreAddRef(t, env, env.UID, hash, "integrity-ref", now)
			tc.mutate(t, workspaceBlobStoreFinalPath(env.BlobRoot, env.UID, hash))

			require.Error(t, store.ReconcileAndGC(context.Background(), env.UID, now))
			record := workspaceBlobStoreRequireBlob(t, env, env.UID, hash)
			require.Equal(t, int64(1), record.RefCount)
			var refs int64
			require.NoError(t, env.UserDB(env.UID).Model(&model.WorkspaceBlobRef{}).
				Where("content_hash = ?", string(hash)).Count(&refs).Error)
			require.Equal(t, int64(1), refs)
		})
	}
}

func TestWorkspaceBlobStoreGCLeavesReferencedAndGracePeriodBlobs(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	store := NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot))
	now := time.Now().UTC().Truncate(time.Second)
	referencedContent := []byte("referenced old file")
	referencedHash := workspaceBlobStoreHash(referencedContent)
	graceContent := []byte("unreferenced in grace")
	graceHash := workspaceBlobStoreHash(graceContent)

	require.NoError(t, store.Put(context.Background(), env.UID, referencedHash, uint64(len(referencedContent)), bytes.NewReader(referencedContent)))
	require.NoError(t, store.Put(context.Background(), env.UID, graceHash, uint64(len(graceContent)), bytes.NewReader(graceContent)))
	workspaceBlobStoreAddRef(t, env, env.UID, referencedHash, "gc-ref", now.Add(-3*time.Hour))
	workspaceBlobStoreExpireBlob(t, env, env.UID, graceHash, now.Add(-30*time.Minute))
	for _, hash := range []dto.WorkspaceContentHash{referencedHash, graceHash} {
		path := workspaceBlobStoreFinalPath(env.BlobRoot, env.UID, hash)
		require.NoError(t, os.Chtimes(path, now.Add(-3*time.Hour), now.Add(-3*time.Hour)))
	}

	require.NoError(t, store.ReconcileAndGC(context.Background(), env.UID, now))
	require.FileExists(t, workspaceBlobStoreFinalPath(env.BlobRoot, env.UID, referencedHash))
	require.FileExists(t, workspaceBlobStoreFinalPath(env.BlobRoot, env.UID, graceHash))
	require.Equal(t, int64(1), workspaceBlobStoreRequireBlob(t, env, env.UID, referencedHash).RefCount)
	require.Zero(t, workspaceBlobStoreRequireBlob(t, env, env.UID, graceHash).RefCount)
}

func TestWorkspaceBlobStoreGCClaimsZeroRefExpiredRowBeforeFileRemoval(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	cfg := workspaceBlobStoreConfig(t, env.BlobRoot)
	plainStore := NewWorkspaceBlobStore(env.WorkspaceRepo, cfg)
	now := time.Now().UTC().Truncate(time.Second)
	content := []byte("expired physical blob")
	hash := workspaceBlobStoreHash(content)
	require.NoError(t, plainStore.Put(context.Background(), env.UID, hash, uint64(len(content)), bytes.NewReader(content)))
	workspaceBlobStoreExpireBlob(t, env, env.UID, hash, now.Add(-2*time.Hour))
	path := workspaceBlobStoreFinalPath(env.BlobRoot, env.UID, hash)
	require.NoError(t, os.Chtimes(path, now.Add(-2*time.Hour), now.Add(-2*time.Hour)))

	events := &workspaceBlobStoreEvents{}
	repo := &workspaceBlobStoreRecordingRepository{WorkspaceRepository: env.WorkspaceRepo, events: events}
	filesystem := &workspaceBlobStoreRecordingFS{
		events: events, trackedFinal: path,
		onRemove: func(removedPath string) error {
			if removedPath != path {
				return nil
			}
			err := env.WorkspaceRepo.Read(context.Background(), env.UID, func(tx domain.WorkspaceReadTx) error {
				_, readErr := tx.Blob(hash)
				return readErr
			})
			if !errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
				return fmt.Errorf("blob row still visible when final removal began: %v", err)
			}
			addErr := env.WorkspaceRepo.Write(context.Background(), env.UID, func(tx domain.WorkspaceWriteTx) error {
				return tx.AddBlobRef(domain.WorkspaceBlobRefRecord{
					ContentHash: hash, OwnerType: "path", OwnerKey: "post-claim-ref",
				}, now)
			})
			if !errors.Is(addErr, domain.ErrWorkspaceRecordNotFound) {
				return fmt.Errorf("post-claim AddBlobRef must fail with missing row, got %v", addErr)
			}
			return nil
		},
	}
	store := newWorkspaceBlobStore(repo, cfg, filesystem)
	require.NoError(t, store.ReconcileAndGC(context.Background(), env.UID, now))

	require.Equal(t, []string{"delete_blob", "remove_final", "sync_fanout"}, events.snapshot())
	require.NoFileExists(t, path)
	workspaceBlobStoreRequireNoBlob(t, env, env.UID, hash)

	crashContent := []byte("post-claim-pre-unlink")
	crashHash := workspaceBlobStoreHash(crashContent)
	require.NoError(t, plainStore.Put(context.Background(), env.UID, crashHash, uint64(len(crashContent)), bytes.NewReader(crashContent)))
	workspaceBlobStoreExpireBlob(t, env, env.UID, crashHash, now.Add(-2*time.Hour))
	crashPath := workspaceBlobStoreFinalPath(env.BlobRoot, env.UID, crashHash)
	require.NoError(t, os.Chtimes(crashPath, now.Add(-2*time.Hour), now.Add(-2*time.Hour)))
	require.NoError(t, env.WorkspaceRepo.Write(context.Background(), env.UID, func(tx domain.WorkspaceWriteTx) error {
		return tx.DeleteBlob(crashHash)
	}))
	require.FileExists(t, crashPath)
	require.NoError(t, plainStore.ReconcileAndGC(context.Background(), env.UID, now))
	require.NoFileExists(t, crashPath)
}

func TestWorkspaceBlobStoreRejectsSymlinkComponentsWithoutTouchingTargets(t *testing.T) {
	content := []byte("must stay inside the CAS root")
	hash := workspaceBlobStoreHash(content)

	t.Run("fanout", func(t *testing.T) {
		env := testutil.NewWorkspaceEnv(t)
		outside := t.TempDir()
		blake3Root := filepath.Join(env.BlobRoot, "user_41", "blake3")
		require.NoError(t, os.MkdirAll(blake3Root, 0700))
		fanout := strings.TrimPrefix(string(hash), "blake3:")[:2]
		require.NoError(t, os.Symlink(outside, filepath.Join(blake3Root, fanout)))
		store := NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot))

		require.Error(t, store.Put(context.Background(), env.UID, hash, uint64(len(content)), bytes.NewReader(content)))
		require.NoFileExists(t, filepath.Join(outside, strings.TrimPrefix(string(hash), "blake3:")))
	})

	t.Run("staging", func(t *testing.T) {
		env := testutil.NewWorkspaceEnv(t)
		outside := t.TempDir()
		blake3Root := filepath.Join(env.BlobRoot, "user_41", "blake3")
		require.NoError(t, os.MkdirAll(blake3Root, 0700))
		require.NoError(t, os.Symlink(outside, filepath.Join(blake3Root, ".tmp")))
		store := NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot))

		require.Error(t, store.Put(context.Background(), env.UID, hash, uint64(len(content)), bytes.NewReader(content)))
		entries, err := os.ReadDir(outside)
		require.NoError(t, err)
		require.Empty(t, entries)
	})

	t.Run("final", func(t *testing.T) {
		env := testutil.NewWorkspaceEnv(t)
		outsideDir := t.TempDir()
		outsidePath := filepath.Join(outsideDir, "outside")
		require.NoError(t, os.WriteFile(outsidePath, content, 0600))
		finalPath := workspaceBlobStoreFinalPath(env.BlobRoot, env.UID, hash)
		require.NoError(t, os.MkdirAll(filepath.Dir(finalPath), 0700))
		require.NoError(t, os.Symlink(outsidePath, finalPath))
		store := NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot))

		require.Error(t, store.Put(context.Background(), env.UID, hash, uint64(len(content)), bytes.NewReader(content)))
		require.Equal(t, content, workspaceBlobStoreReadFile(t, outsidePath))
		require.FileExists(t, outsidePath)
	})
}

func TestWorkspaceBlobStoreReleasesKeyedLocksAfterUniqueHashChurn(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	store := NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot))
	const blobCount = 48
	for i := range blobCount {
		content := []byte(fmt.Sprintf("unique-lock-%03d", i))
		hash := workspaceBlobStoreHash(content)
		require.NoError(t, store.Put(context.Background(), env.UID, hash, uint64(len(content)), bytes.NewReader(content)))
	}

	implementation := store.(*workspaceBlobStore)
	lockCount := 0
	implementation.contentLock.Range(func(_, _ any) bool {
		lockCount++
		return true
	})
	require.Zero(t, lockCount, "keyed locks must be removed after their last holder/waiter releases")
}

func TestWorkspaceBlobStoreSweepDeletesOnlyExpiredStagingFiles(t *testing.T) {
	env := testutil.NewWorkspaceEnv(t)
	store := NewWorkspaceBlobStore(env.WorkspaceRepo, workspaceBlobStoreConfig(t, env.BlobRoot))
	now := time.Now().UTC().Truncate(time.Second)
	cutoff := now.Add(-time.Hour)
	stageDir := workspaceBlobStoreStageDir(env.BlobRoot, env.UID)
	require.NoError(t, os.MkdirAll(stageDir, 0700))

	expired := filepath.Join(stageDir, "blob-expired")
	recent := filepath.Join(stageDir, "blob-recent")
	boundary := filepath.Join(stageDir, "blob-boundary")
	unrelated := filepath.Join(stageDir, "unrelated-expired")
	stageDirectory := filepath.Join(stageDir, "blob-directory")
	for _, path := range []string{expired, recent, boundary, unrelated} {
		require.NoError(t, os.WriteFile(path, []byte(path), 0600))
	}
	require.NoError(t, os.Mkdir(stageDirectory, 0700))
	require.NoError(t, os.Chtimes(expired, cutoff.Add(-time.Second), cutoff.Add(-time.Second)))
	require.NoError(t, os.Chtimes(recent, now, now))
	require.NoError(t, os.Chtimes(boundary, cutoff, cutoff))
	require.NoError(t, os.Chtimes(unrelated, cutoff.Add(-time.Second), cutoff.Add(-time.Second)))
	require.NoError(t, os.Chtimes(stageDirectory, cutoff.Add(-time.Second), cutoff.Add(-time.Second)))

	require.NoError(t, store.ReconcileAndGC(context.Background(), env.UID, now))
	require.NoFileExists(t, expired)
	for _, path := range []string{recent, boundary, unrelated} {
		require.FileExists(t, path)
	}
	require.DirExists(t, stageDirectory)
}

type workspaceBlobStoreMultiReader struct {
	data      []byte
	chunkSize int
	offset    int
	reads     int
}

func (r *workspaceBlobStoreMultiReader) Read(p []byte) (int, error) {
	r.reads++
	if r.offset == len(r.data) {
		return 0, io.EOF
	}
	n := min(len(p), r.chunkSize, len(r.data)-r.offset)
	copy(p, r.data[r.offset:r.offset+n])
	r.offset += n
	return n, nil
}

type workspaceBlobStoreFailReader struct {
	data   []byte
	offset int
}

func (r *workspaceBlobStoreFailReader) Read(p []byte) (int, error) {
	if r.offset == len(r.data) {
		return 0, errors.New("injected reader failure")
	}
	n := copy(p, r.data[r.offset:])
	r.offset += n
	return n, nil
}

type workspaceBlobStoreEvents struct {
	mu     sync.Mutex
	events []string
}

func (e *workspaceBlobStoreEvents) add(event string) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.events = append(e.events, event)
}

func (e *workspaceBlobStoreEvents) snapshot() []string {
	e.mu.Lock()
	defer e.mu.Unlock()
	return append([]string(nil), e.events...)
}

type workspaceBlobStoreRecordingFS struct {
	events          *workspaceBlobStoreEvents
	created         *workspaceBlobStoreRecordingFile
	opened          *workspaceBlobStoreRecordingFile
	trackedFinal    string
	onRemove        func(string) error
	openOverride    func(string) (workspaceBlobFile, error)
	failStageSync   error
	failStageClose  error
	failRename      error
	failDirSync     error
	renameSawClosed atomic.Bool
}

func (f *workspaceBlobStoreRecordingFS) CreateTemp(dir, pattern string) (workspaceBlobFile, error) {
	created, err := os.CreateTemp(dir, pattern)
	if err == nil {
		f.created = &workspaceBlobStoreRecordingFile{
			workspaceBlobFile: created,
			events:            f.events,
			failSync:          f.failStageSync,
			failClose:         f.failStageClose,
		}
		f.events.add("create_temp")
	}
	return f.created, err
}

func (f *workspaceBlobStoreRecordingFS) Open(name string) (workspaceBlobFile, error) {
	if f.openOverride != nil {
		return f.openOverride(name)
	}
	return os.Open(name)
}

func (*workspaceBlobStoreRecordingFS) Lstat(name string) (os.FileInfo, error) {
	return os.Lstat(name)
}

func (*workspaceBlobStoreRecordingFS) MkdirAll(path string, perm fs.FileMode) error {
	return os.MkdirAll(path, perm)
}

func (f *workspaceBlobStoreRecordingFS) Rename(oldPath, newPath string) error {
	if f.created != nil && f.created.closed.Load() {
		f.renameSawClosed.Store(true)
	}
	f.events.add("rename")
	if f.failRename != nil {
		return f.failRename
	}
	return os.Rename(oldPath, newPath)
}

func (f *workspaceBlobStoreRecordingFS) Remove(path string) error {
	if path == f.trackedFinal {
		f.events.add("remove_final")
	}
	if f.onRemove != nil {
		if err := f.onRemove(path); err != nil {
			return err
		}
	}
	return os.Remove(path)
}

func (*workspaceBlobStoreRecordingFS) ReadDir(path string) ([]os.DirEntry, error) {
	return os.ReadDir(path)
}

func (f *workspaceBlobStoreRecordingFS) SyncDir(path string) error {
	if filepath.Base(path) != ".tmp" {
		f.events.add("sync_fanout")
		if f.failDirSync != nil {
			return f.failDirSync
		}
	}
	dir, err := os.Open(path)
	if err != nil {
		return err
	}
	defer dir.Close()
	return dir.Sync()
}

type workspaceBlobStoreRecordingFile struct {
	workspaceBlobFile
	events    *workspaceBlobStoreEvents
	failSync  error
	failClose error
	reads     atomic.Int64
	closed    atomic.Bool
}

func (f *workspaceBlobStoreRecordingFile) Read(p []byte) (int, error) {
	f.reads.Add(1)
	return f.workspaceBlobFile.Read(p)
}

func (f *workspaceBlobStoreRecordingFile) Sync() error {
	if f.events != nil {
		f.events.add("stage_sync")
	}
	if f.failSync != nil {
		return f.failSync
	}
	return f.workspaceBlobFile.Sync()
}

func (f *workspaceBlobStoreRecordingFile) Close() error {
	if !f.closed.CompareAndSwap(false, true) {
		return f.workspaceBlobFile.Close()
	}
	if f.events != nil {
		f.events.add("stage_close")
	}
	closeErr := f.workspaceBlobFile.Close()
	if f.failClose != nil {
		return f.failClose
	}
	return closeErr
}

type workspaceBlobStoreRecordingRepository struct {
	domain.WorkspaceRepository
	events       *workspaceBlobStoreEvents
	failSaveBlob error
}

func (r *workspaceBlobStoreRecordingRepository) Write(
	ctx context.Context,
	uid int64,
	fn func(domain.WorkspaceWriteTx) error,
) error {
	return r.WorkspaceRepository.Write(ctx, uid, func(tx domain.WorkspaceWriteTx) error {
		return fn(&workspaceBlobStoreRecordingWriteTx{
			WorkspaceWriteTx: tx,
			events:           r.events,
			failSaveBlob:     r.failSaveBlob,
		})
	})
}

type workspaceBlobStoreRecordingWriteTx struct {
	domain.WorkspaceWriteTx
	events       *workspaceBlobStoreEvents
	failSaveBlob error
}

func (tx *workspaceBlobStoreRecordingWriteTx) SaveBlob(record domain.WorkspaceBlobRecord) error {
	tx.events.add("save_blob")
	if tx.failSaveBlob != nil {
		return tx.failSaveBlob
	}
	return tx.WorkspaceWriteTx.SaveBlob(record)
}

func (tx *workspaceBlobStoreRecordingWriteTx) DeleteBlob(hash dto.WorkspaceContentHash) error {
	tx.events.add("delete_blob")
	return tx.WorkspaceWriteTx.DeleteBlob(hash)
}

func (tx *workspaceBlobStoreRecordingWriteTx) ClaimBlobForGC(
	hash dto.WorkspaceContentHash,
	unreferencedBefore time.Time,
) (bool, error) {
	tx.events.add("delete_blob")
	return tx.WorkspaceWriteTx.ClaimBlobForGC(hash, unreferencedBefore)
}

func workspaceBlobStoreConfig(t *testing.T, root string) *config.WorkspaceConfig {
	t.Helper()
	cfg := &config.WorkspaceConfig{
		BlobPath: root, MaxPaths: 50_000, MaxBytes: dto.WorkspaceMaxBlobBytes,
		EventRetention: "30d", EventMaxPerWorkspace: 100_000,
		BlobGCGrace: "1h", StagingTTL: "1h", PruneBatchSize: 500,
		MaxWorkspacesPerUser: config.WorkspaceMaxPerUser,
	}
	require.NoError(t, cfg.Validate())
	return cfg
}

func workspaceBlobStoreHash(content []byte) dto.WorkspaceContentHash {
	digest := blake3.Sum256(content)
	return dto.WorkspaceContentHash(fmt.Sprintf("blake3:%x", digest))
}

func workspaceBlobStoreFinalPath(root string, uid int64, hash dto.WorkspaceContentHash) string {
	digest := strings.TrimPrefix(string(hash), "blake3:")
	return filepath.Join(root, "user_"+strconv.FormatInt(uid, 10), "blake3", digest[:2], digest)
}

func workspaceBlobStoreStageDir(root string, uid int64) string {
	return filepath.Join(root, "user_"+strconv.FormatInt(uid, 10), "blake3", ".tmp")
}

func workspaceBlobStoreWriteFinal(
	t *testing.T,
	root string,
	uid int64,
	hash dto.WorkspaceContentHash,
	content []byte,
) string {
	t.Helper()
	path := workspaceBlobStoreFinalPath(root, uid, hash)
	require.NoError(t, os.MkdirAll(filepath.Dir(path), 0700))
	require.NoError(t, os.WriteFile(path, content, 0600))
	return path
}

func workspaceBlobStoreReadFile(t *testing.T, path string) []byte {
	t.Helper()
	content, err := os.ReadFile(path)
	require.NoError(t, err)
	return content
}

func workspaceBlobStoreReadDir(t *testing.T, path string) []os.DirEntry {
	t.Helper()
	entries, err := os.ReadDir(path)
	require.NoError(t, err)
	return entries
}

func workspaceBlobStoreStageEntriesIfPresent(t *testing.T, root string, uid int64) []os.DirEntry {
	t.Helper()
	entries, err := os.ReadDir(workspaceBlobStoreStageDir(root, uid))
	if errors.Is(err, fs.ErrNotExist) {
		return nil
	}
	require.NoError(t, err)
	return entries
}

func workspaceBlobStoreRequireBlob(
	t *testing.T,
	env *testutil.WorkspaceEnv,
	uid int64,
	hash dto.WorkspaceContentHash,
) *domain.WorkspaceBlobRecord {
	t.Helper()
	var record *domain.WorkspaceBlobRecord
	require.NoError(t, env.WorkspaceRepo.Read(context.Background(), uid, func(tx domain.WorkspaceReadTx) error {
		var err error
		record, err = tx.Blob(hash)
		return err
	}))
	require.NotNil(t, record)
	return record
}

func workspaceBlobStoreRequireNoBlob(
	t *testing.T,
	env *testutil.WorkspaceEnv,
	uid int64,
	hash dto.WorkspaceContentHash,
) {
	t.Helper()
	err := env.WorkspaceRepo.Read(context.Background(), uid, func(tx domain.WorkspaceReadTx) error {
		_, err := tx.Blob(hash)
		return err
	})
	require.ErrorIs(t, err, domain.ErrWorkspaceRecordNotFound)
}

func workspaceBlobStoreExpireBlob(
	t *testing.T,
	env *testutil.WorkspaceEnv,
	uid int64,
	hash dto.WorkspaceContentHash,
	unreferencedAt time.Time,
) {
	t.Helper()
	require.NoError(t, env.UserDB(uid).Model(&model.WorkspaceBlob{}).
		Where("content_hash = ?", string(hash)).
		Updates(map[string]any{"ref_count": 0, "unreferenced_at": unreferencedAt}).Error)
}

func workspaceBlobStoreAddRef(
	t *testing.T,
	env *testutil.WorkspaceEnv,
	uid int64,
	hash dto.WorkspaceContentHash,
	ownerKey string,
	now time.Time,
) {
	t.Helper()
	require.NoError(t, env.WorkspaceRepo.Write(context.Background(), uid, func(tx domain.WorkspaceWriteTx) error {
		return tx.AddBlobRef(domain.WorkspaceBlobRefRecord{
			ContentHash: hash,
			OwnerType:   "path",
			OwnerKey:    ownerKey,
		}, now)
	}))
}
