package service

import (
	"context"
	"encoding/hex"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/haierkeys/fast-note-sync-service/internal/config"
	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/pkg/util"
	"github.com/zeebo/blake3"
	"go.uber.org/zap"
)

var ErrWorkspaceBlobIntegrity = errors.New("workspace blob integrity error")

type WorkspaceBlobStore interface {
	Has(ctx context.Context, uid int64, hash dto.WorkspaceContentHash, size uint64) (bool, error)
	Put(
		ctx context.Context,
		uid int64,
		hash dto.WorkspaceContentHash,
		size uint64,
		src io.Reader,
	) error
	Open(ctx context.Context, uid int64, hash dto.WorkspaceContentHash) (io.ReadCloser, uint64, error)
	ReconcileAndGC(ctx context.Context, uid int64, now time.Time) error
}

type workspaceBlobFS interface {
	CreateTemp(dir, pattern string) (workspaceBlobFile, error)
	Open(name string) (workspaceBlobFile, error)
	Lstat(name string) (os.FileInfo, error)
	MkdirAll(path string, perm fs.FileMode) error
	Rename(oldPath, newPath string) error
	Remove(path string) error
	ReadDir(path string) ([]os.DirEntry, error)
	SyncDir(path string) error
}

type workspaceBlobFile interface {
	io.Reader
	io.Writer
	io.Seeker
	Chmod(fs.FileMode) error
	Close() error
	Name() string
	Stat() (os.FileInfo, error)
	Sync() error
}

type workspaceBlobStore struct {
	repo          domain.WorkspaceRepository
	fs            workspaceBlobFS
	blobRoot      string
	gcGrace       time.Duration
	stagingTTL    time.Duration
	pageSize      int
	initErr       error
	contentLockMu sync.Mutex
	contentLock   sync.Map
}

// InitError exposes constructor validation to the application wiring layer.
func (s *workspaceBlobStore) InitError() error {
	if s == nil {
		return errors.New("workspace blob store is nil")
	}
	return s.initErr
}

type osWorkspaceBlobFS struct{}

type workspaceBlobKeyedLock struct {
	mutex sync.Mutex
	refs  int
}

type workspaceBlobFinal struct {
	hash dto.WorkspaceContentHash
	path string
	info os.FileInfo
}

func NewWorkspaceBlobStore(
	repo domain.WorkspaceRepository,
	cfg *config.WorkspaceConfig,
) WorkspaceBlobStore {
	return newWorkspaceBlobStore(repo, cfg, osWorkspaceBlobFS{})
}

func newWorkspaceBlobStore(
	repo domain.WorkspaceRepository,
	cfg *config.WorkspaceConfig,
	filesystem workspaceBlobFS,
) WorkspaceBlobStore {
	store := &workspaceBlobStore{repo: repo, fs: filesystem}
	if repo == nil {
		store.initErr = errors.New("workspace blob repository is nil")
		return store
	}
	if filesystem == nil {
		store.initErr = errors.New("workspace blob filesystem is nil")
		return store
	}
	if cfg == nil {
		store.initErr = errors.New("workspace blob config is nil")
		return store
	}
	if err := cfg.Validate(); err != nil {
		store.initErr = err
		return store
	}
	gcGrace, err := util.ParseDuration(cfg.BlobGCGrace)
	if err != nil {
		store.initErr = fmt.Errorf("parse workspace blob GC grace: %w", err)
		return store
	}
	stagingTTL, err := util.ParseDuration(cfg.StagingTTL)
	if err != nil {
		store.initErr = fmt.Errorf("parse workspace blob staging TTL: %w", err)
		return store
	}
	store.blobRoot = cfg.BlobPath
	store.gcGrace = gcGrace
	store.stagingTTL = stagingTTL
	store.pageSize = cfg.PruneBatchSize
	return store
}

func (s *workspaceBlobStore) Has(
	ctx context.Context,
	uid int64,
	hash dto.WorkspaceContentHash,
	size uint64,
) (bool, error) {
	if err := s.validateOperation(ctx, uid); err != nil {
		return false, err
	}
	canonical, err := workspaceBlobIdentity(hash, size)
	if err != nil {
		return false, err
	}
	if err := s.repo.Migrate(ctx, uid); err != nil {
		return false, err
	}
	var record *domain.WorkspaceBlobRecord
	err = s.repo.Read(ctx, uid, func(tx domain.WorkspaceReadTx) error {
		var readErr error
		record, readErr = tx.Blob(canonical)
		return readErr
	})
	if errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
		return false, nil
	}
	if err != nil {
		return false, err
	}
	return record.Size == size, nil
}

func (s *workspaceBlobStore) Put(
	ctx context.Context,
	uid int64,
	hash dto.WorkspaceContentHash,
	size uint64,
	src io.Reader,
) error {
	if err := s.validateOperation(ctx, uid); err != nil {
		return err
	}
	canonical, err := workspaceBlobIdentity(hash, size)
	if err != nil {
		return err
	}
	if src == nil {
		return errors.New("workspace blob source is nil")
	}
	unlock := s.lock(uid, canonical)
	defer unlock()
	if err := s.repo.Migrate(ctx, uid); err != nil {
		return err
	}

	finalPath, stagePattern := workspaceBlobPaths(s.blobRoot, uid, canonical)
	if err := s.prepareBlobDirectories(uid, canonical); err != nil {
		return err
	}
	stageDir := filepath.Dir(stagePattern)
	stage, err := s.fs.CreateTemp(stageDir, filepath.Base(stagePattern))
	if err != nil {
		return fmt.Errorf("create workspace blob stage: %w", err)
	}
	stagePath := stage.Name()
	stageOpen := true
	ownedStage := true
	defer func() {
		if stageOpen {
			_ = stage.Close()
		}
		if ownedStage {
			_ = s.fs.Remove(stagePath)
		}
	}()
	if err := stage.Chmod(0600); err != nil {
		return fmt.Errorf("set workspace blob stage permissions: %w", err)
	}

	hasher := blake3.New()
	utf8Validator := newWorkspaceUTF8Validator()
	limited := io.LimitReader(&workspaceBlobContextReader{ctx: ctx, reader: src}, int64(size)+1)
	written, err := io.Copy(io.MultiWriter(stage, hasher, utf8Validator), limited)
	if err != nil {
		return fmt.Errorf("stream workspace blob: %w", err)
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	if written != int64(size) {
		return fmt.Errorf("workspace blob size mismatch: declared %d, received %d", size, written)
	}
	actualHash := dto.WorkspaceContentHash("blake3:" + hex.EncodeToString(hasher.Sum(nil)))
	if actualHash != canonical {
		return fmt.Errorf("workspace blob hash mismatch: declared %s, computed %s", canonical, actualHash)
	}
	utf8Valid := utf8Validator.Valid()
	if err := stage.Sync(); err != nil {
		return fmt.Errorf("sync workspace blob stage: %w", err)
	}
	if err := stage.Close(); err != nil {
		stageOpen = false
		return fmt.Errorf("close workspace blob stage: %w", err)
	}
	stageOpen = false

	if exists, err := s.canonicalFinalExists(finalPath); err != nil {
		return err
	} else if exists {
		valid, verifyErr := s.verifyFinal(ctx, finalPath, canonical, size)
		if verifyErr != nil {
			return verifyErr
		}
		if valid != utf8Valid {
			return workspaceBlobIntegrity("existing final UTF-8 metadata changed for %s", canonical)
		}
		if err := s.fs.Remove(stagePath); err != nil {
			return fmt.Errorf("remove deduplicated workspace blob stage: %w", err)
		}
		ownedStage = false
		if err := s.fs.SyncDir(filepath.Dir(finalPath)); err != nil {
			return fmt.Errorf("sync workspace blob fanout directory: %w", err)
		}
		return s.ensureBlobRow(ctx, uid, canonical, size, utf8Valid, time.Now().UTC())
	}

	if err := s.fs.Rename(stagePath, finalPath); err != nil {
		return fmt.Errorf("publish workspace blob final: %w", err)
	}
	ownedStage = false
	if err := s.fs.SyncDir(filepath.Dir(finalPath)); err != nil {
		return fmt.Errorf("sync workspace blob fanout directory: %w", err)
	}
	return s.ensureBlobRow(ctx, uid, canonical, size, utf8Valid, time.Now().UTC())
}

func (s *workspaceBlobStore) Open(
	ctx context.Context,
	uid int64,
	hash dto.WorkspaceContentHash,
) (io.ReadCloser, uint64, error) {
	if err := s.validateOperation(ctx, uid); err != nil {
		return nil, 0, err
	}
	canonical, err := dto.ParseWorkspaceContentHash(string(hash))
	if err != nil {
		return nil, 0, err
	}
	unlock := s.lock(uid, canonical)
	defer unlock()
	if err := s.repo.Migrate(ctx, uid); err != nil {
		return nil, 0, err
	}
	var record *domain.WorkspaceBlobRecord
	if err := s.repo.Read(ctx, uid, func(tx domain.WorkspaceReadTx) error {
		var readErr error
		record, readErr = tx.Blob(canonical)
		return readErr
	}); err != nil {
		return nil, 0, err
	}
	if record.Size > dto.WorkspaceMaxBlobBytes {
		return nil, 0, workspaceBlobIntegrity("stored size exceeds limit for %s", canonical)
	}
	if exists, err := s.validateBlobDirectories(uid, canonical); err != nil {
		return nil, 0, err
	} else if !exists {
		return nil, 0, workspaceBlobIntegrity("missing directory for %s", canonical)
	}
	finalPath, _ := workspaceBlobPaths(s.blobRoot, uid, canonical)
	file, valid, err := s.openVerified(ctx, finalPath, canonical, record.Size)
	if err != nil {
		return nil, 0, err
	}
	if valid != record.UTF8Valid {
		_ = file.Close()
		return nil, 0, workspaceBlobIntegrity("stored UTF-8 metadata mismatch for %s", canonical)
	}
	return file, record.Size, nil
}

func (s *workspaceBlobStore) ReconcileAndGC(ctx context.Context, uid int64, now time.Time) error {
	if err := s.validateOperation(ctx, uid); err != nil {
		return err
	}
	if err := s.repo.Migrate(ctx, uid); err != nil {
		return err
	}
	if err := s.sweepStaging(ctx, uid, now.Add(-s.stagingTTL)); err != nil {
		return err
	}
	if err := s.repo.Write(ctx, uid, func(tx domain.WorkspaceWriteTx) error {
		return tx.ReconcileBlobRefCounts(now)
	}); err != nil {
		return err
	}
	if err := s.reconcileOrphanFinals(ctx, uid, now.Add(-s.gcGrace)); err != nil {
		return err
	}
	return s.reconcileBlobRows(ctx, uid, now.Add(-s.gcGrace))
}

func (s *workspaceBlobStore) validateOperation(ctx context.Context, uid int64) error {
	if s.initErr != nil {
		return s.initErr
	}
	if uid <= 0 {
		return domain.ErrWorkspaceInvalidUID
	}
	return ctx.Err()
}

func workspaceBlobIdentity(
	hash dto.WorkspaceContentHash,
	size uint64,
) (dto.WorkspaceContentHash, error) {
	canonical, err := dto.ParseWorkspaceContentHash(string(hash))
	if err != nil {
		return "", err
	}
	if size > dto.WorkspaceMaxBlobBytes {
		return "", fmt.Errorf("workspace blob size exceeds limit %d", dto.WorkspaceMaxBlobBytes)
	}
	return canonical, nil
}

func workspaceBlobPaths(
	blobRoot string,
	uid int64,
	hash dto.WorkspaceContentHash,
) (finalPath, stagePattern string) {
	digest := strings.TrimPrefix(string(hash), "blake3:")
	userRoot := filepath.Join(blobRoot, "user_"+strconv.FormatInt(uid, 10), "blake3")
	return filepath.Join(userRoot, digest[:2], digest), filepath.Join(userRoot, ".tmp", "blob-*")
}

func workspaceBlobDirectoryPaths(
	blobRoot string,
	uid int64,
	hash dto.WorkspaceContentHash,
) (userRoot, algorithmRoot, fanout, staging string) {
	digest := strings.TrimPrefix(string(hash), "blake3:")
	userRoot = filepath.Join(blobRoot, "user_"+strconv.FormatInt(uid, 10))
	algorithmRoot = filepath.Join(userRoot, "blake3")
	return userRoot, algorithmRoot, filepath.Join(algorithmRoot, digest[:2]), filepath.Join(algorithmRoot, ".tmp")
}

func (s *workspaceBlobStore) prepareBlobDirectories(uid int64, hash dto.WorkspaceContentHash) error {
	userRoot, algorithmRoot, fanout, staging := workspaceBlobDirectoryPaths(s.blobRoot, uid, hash)
	for _, path := range []string{s.blobRoot, userRoot, algorithmRoot, fanout, staging} {
		if _, err := s.ensureSafeDirectory(path, true); err != nil {
			return err
		}
	}
	return nil
}

func (s *workspaceBlobStore) validateBlobDirectories(
	uid int64,
	hash dto.WorkspaceContentHash,
) (bool, error) {
	userRoot, algorithmRoot, fanout, _ := workspaceBlobDirectoryPaths(s.blobRoot, uid, hash)
	for _, path := range []string{s.blobRoot, userRoot, algorithmRoot, fanout} {
		exists, err := s.ensureSafeDirectory(path, false)
		if err != nil || !exists {
			return false, err
		}
	}
	return true, nil
}

func (s *workspaceBlobStore) ensureSafeDirectory(path string, create bool) (bool, error) {
	info, err := s.fs.Lstat(path)
	if errors.Is(err, fs.ErrNotExist) && create {
		if err := s.fs.MkdirAll(path, 0700); err != nil {
			return false, fmt.Errorf("create workspace blob directory %s: %w", path, err)
		}
		info, err = s.fs.Lstat(path)
	}
	if errors.Is(err, fs.ErrNotExist) {
		return false, nil
	}
	if err != nil {
		return false, fmt.Errorf("lstat workspace blob directory %s: %w", path, err)
	}
	if info.Mode()&os.ModeSymlink != 0 || !info.IsDir() {
		return false, workspaceBlobIntegrity("unsafe workspace blob directory %s", path)
	}
	return true, nil
}

func (s *workspaceBlobStore) lock(uid int64, hash dto.WorkspaceContentHash) func() {
	key := strconv.FormatInt(uid, 10) + "/" + string(hash)
	s.contentLockMu.Lock()
	actual, ok := s.contentLock.Load(key)
	if !ok {
		actual = &workspaceBlobKeyedLock{}
		s.contentLock.Store(key, actual)
	}
	entry := actual.(*workspaceBlobKeyedLock)
	entry.refs++
	s.contentLockMu.Unlock()

	entry.mutex.Lock()
	return func() {
		entry.mutex.Unlock()
		s.contentLockMu.Lock()
		entry.refs--
		if entry.refs == 0 {
			s.contentLock.Delete(key)
		}
		s.contentLockMu.Unlock()
	}
}

func (s *workspaceBlobStore) ensureBlobRow(
	ctx context.Context,
	uid int64,
	hash dto.WorkspaceContentHash,
	size uint64,
	utf8Valid bool,
	now time.Time,
) error {
	return s.repo.Write(ctx, uid, func(tx domain.WorkspaceWriteTx) error {
		existing, err := tx.Blob(hash)
		if err == nil {
			if existing.Size != size {
				return workspaceBlobIntegrity("stored size mismatch for %s", hash)
			}
			if existing.UTF8Valid != utf8Valid {
				existing.UTF8Valid = utf8Valid
				return tx.SaveBlob(*existing)
			}
			return nil
		}
		if !errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
			return err
		}
		return tx.SaveBlob(domain.WorkspaceBlobRecord{
			ContentHash:    hash,
			Size:           size,
			UTF8Valid:      utf8Valid,
			UnreferencedAt: &now,
		})
	})
}

func (s *workspaceBlobStore) verifyFinal(
	ctx context.Context,
	path string,
	hash dto.WorkspaceContentHash,
	size uint64,
) (bool, error) {
	file, valid, err := s.openVerified(ctx, path, hash, size)
	if file != nil {
		closeErr := file.Close()
		if err == nil && closeErr != nil {
			err = fmt.Errorf("close workspace blob final: %w", closeErr)
		}
	}
	return valid, err
}

func (s *workspaceBlobStore) openVerified(
	ctx context.Context,
	path string,
	hash dto.WorkspaceContentHash,
	size uint64,
) (workspaceBlobFile, bool, error) {
	preOpenInfo, err := s.fs.Lstat(path)
	if err != nil {
		if errors.Is(err, fs.ErrNotExist) {
			return nil, false, workspaceBlobIntegrity("missing final for %s", hash)
		}
		return nil, false, fmt.Errorf("lstat workspace blob final: %w", err)
	}
	if preOpenInfo.Mode()&os.ModeSymlink != 0 || !preOpenInfo.Mode().IsRegular() {
		return nil, false, workspaceBlobIntegrity("unsafe final type for %s", hash)
	}
	file, err := s.fs.Open(path)
	if err != nil {
		if errors.Is(err, fs.ErrNotExist) {
			return nil, false, workspaceBlobIntegrity("missing final for %s", hash)
		}
		return nil, false, fmt.Errorf("open workspace blob final: %w", err)
	}
	fail := func(err error) (workspaceBlobFile, bool, error) {
		_ = file.Close()
		return nil, false, err
	}
	info, err := file.Stat()
	if err != nil {
		return fail(fmt.Errorf("stat opened workspace blob final: %w", err))
	}
	if !os.SameFile(preOpenInfo, info) {
		return fail(workspaceBlobIntegrity("workspace blob final changed before open for %s", hash))
	}
	if !info.Mode().IsRegular() || info.Size() != int64(size) {
		return fail(workspaceBlobIntegrity("invalid final size or type for %s", hash))
	}
	hasher := blake3.New()
	validator := newWorkspaceUTF8Validator()
	read, err := io.Copy(
		io.MultiWriter(hasher, validator),
		io.LimitReader(&workspaceBlobContextReader{ctx: ctx, reader: file}, int64(size)+1),
	)
	if err != nil {
		return fail(fmt.Errorf("hash workspace blob final: %w", err))
	}
	if err := ctx.Err(); err != nil {
		return fail(err)
	}
	actual := dto.WorkspaceContentHash("blake3:" + hex.EncodeToString(hasher.Sum(nil)))
	if read != int64(size) || actual != hash {
		return fail(workspaceBlobIntegrity("corrupt final for %s", hash))
	}
	if _, err := file.Seek(0, io.SeekStart); err != nil {
		return fail(fmt.Errorf("rewind workspace blob final: %w", err))
	}
	return file, validator.Valid(), nil
}

func (s *workspaceBlobStore) sweepStaging(
	ctx context.Context,
	uid int64,
	cutoff time.Time,
) error {
	_, stagePattern := workspaceBlobPaths(s.blobRoot, uid, workspaceBlobZeroHash())
	stageDir := filepath.Dir(stagePattern)
	userRoot, algorithmRoot, _, _ := workspaceBlobDirectoryPaths(s.blobRoot, uid, workspaceBlobZeroHash())
	for _, path := range []string{s.blobRoot, userRoot, algorithmRoot, stageDir} {
		exists, err := s.ensureSafeDirectory(path, false)
		if err != nil || !exists {
			return err
		}
	}
	entries, err := s.fs.ReadDir(stageDir)
	if err != nil {
		return fmt.Errorf("read workspace blob staging directory: %w", err)
	}
	removed := 0
	for _, entry := range entries {
		if removed == s.pageSize {
			break
		}
		if err := ctx.Err(); err != nil {
			return err
		}
		if !strings.HasPrefix(entry.Name(), "blob-") || entry.Type()&os.ModeSymlink != 0 || entry.IsDir() {
			continue
		}
		info, err := entry.Info()
		if err != nil {
			if errors.Is(err, fs.ErrNotExist) {
				continue
			}
			return fmt.Errorf("stat workspace blob stage: %w", err)
		}
		if !info.Mode().IsRegular() || !info.ModTime().Before(cutoff) {
			continue
		}
		if err := s.fs.Remove(filepath.Join(stageDir, entry.Name())); err != nil && !errors.Is(err, fs.ErrNotExist) {
			return fmt.Errorf("remove expired workspace blob stage: %w", err)
		}
		removed++
	}
	if removed != 0 {
		if err := s.fs.SyncDir(stageDir); err != nil {
			return fmt.Errorf("sync workspace blob staging directory: %w", err)
		}
	}
	return nil
}

func (s *workspaceBlobStore) reconcileOrphanFinals(
	ctx context.Context,
	uid int64,
	cutoff time.Time,
) error {
	finals, err := s.scanCanonicalFinals(ctx, uid)
	if err != nil {
		return err
	}
	for _, final := range finals {
		if err := ctx.Err(); err != nil {
			return err
		}
		unlock := s.lock(uid, final.hash)
		err := s.repo.Read(ctx, uid, func(tx domain.WorkspaceReadTx) error {
			_, readErr := tx.Blob(final.hash)
			return readErr
		})
		if err == nil {
			unlock()
			continue
		}
		if !errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
			unlock()
			return err
		}
		if !final.info.ModTime().Before(cutoff) {
			unlock()
			continue
		}
		removeErr := s.fs.Remove(final.path)
		if removeErr == nil || errors.Is(removeErr, fs.ErrNotExist) {
			removeErr = s.syncDirIfPresent(filepath.Dir(final.path))
		}
		unlock()
		if removeErr != nil {
			return fmt.Errorf("remove orphan workspace blob final: %w", removeErr)
		}
	}
	return nil
}

func (s *workspaceBlobStore) reconcileBlobRows(
	ctx context.Context,
	uid int64,
	cutoff time.Time,
) error {
	var cursor *dto.WorkspaceContentHash
	for {
		var page []domain.WorkspaceBlobRecord
		if err := s.repo.Read(ctx, uid, func(tx domain.WorkspaceReadTx) error {
			var readErr error
			page, readErr = tx.BlobsAfter(cursor, s.pageSize)
			return readErr
		}); err != nil {
			return err
		}
		if len(page) == 0 {
			return nil
		}
		for i := range page {
			if err := s.reconcileBlobRow(ctx, uid, &page[i], cutoff); err != nil {
				return err
			}
		}
		last := page[len(page)-1].ContentHash
		cursor = &last
		if len(page) < s.pageSize {
			return nil
		}
	}
}

func (s *workspaceBlobStore) reconcileBlobRow(
	ctx context.Context,
	uid int64,
	record *domain.WorkspaceBlobRecord,
	cutoff time.Time,
) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	unlock := s.lock(uid, record.ContentHash)
	defer unlock()
	finalPath, _ := workspaceBlobPaths(s.blobRoot, uid, record.ContentHash)
	if record.RefCount > 0 {
		exists, err := s.canonicalFinalExists(finalPath)
		if err != nil {
			return err
		}
		if !exists {
			return workspaceBlobIntegrity("referenced final missing for %s", record.ContentHash)
		}
		_, err = s.verifyFinal(ctx, finalPath, record.ContentHash, record.Size)
		return err
	}
	if record.RefCount < 0 {
		return workspaceBlobIntegrity("negative reference count for %s", record.ContentHash)
	}
	if record.UnreferencedAt == nil || !record.UnreferencedAt.Before(cutoff) {
		return nil
	}
	claimed := false
	err := s.repo.Write(ctx, uid, func(tx domain.WorkspaceWriteTx) error {
		var claimErr error
		claimed, claimErr = tx.ClaimBlobForGC(record.ContentHash, cutoff)
		return claimErr
	})
	if errors.Is(err, domain.ErrWorkspaceRecordNotFound) {
		return nil
	}
	if err != nil || !claimed {
		return err
	}
	directoriesExist, err := s.validateBlobDirectories(uid, record.ContentHash)
	if err != nil || !directoriesExist {
		return err
	}
	finalExists, err := s.canonicalFinalExists(finalPath)
	if err != nil {
		return err
	}
	if finalExists {
		if err := s.fs.Remove(finalPath); err != nil && !errors.Is(err, fs.ErrNotExist) {
			return fmt.Errorf("remove claimed workspace blob final: %w", err)
		}
	}
	return s.syncDirIfPresent(filepath.Dir(finalPath))
}

func (s *workspaceBlobStore) scanCanonicalFinals(
	ctx context.Context,
	uid int64,
) ([]workspaceBlobFinal, error) {
	userRoot, root, _, _ := workspaceBlobDirectoryPaths(s.blobRoot, uid, workspaceBlobZeroHash())
	for _, path := range []string{s.blobRoot, userRoot, root} {
		exists, err := s.ensureSafeDirectory(path, false)
		if err != nil || !exists {
			return nil, err
		}
	}
	fanouts, err := s.fs.ReadDir(root)
	if err != nil {
		return nil, fmt.Errorf("read workspace blob root: %w", err)
	}
	finals := make([]workspaceBlobFinal, 0)
	for _, fanout := range fanouts {
		if err := ctx.Err(); err != nil {
			return nil, err
		}
		if fanout.Name() == ".tmp" {
			continue
		}
		if !workspaceBlobLowerHex(fanout.Name(), 2) {
			workspaceBlobLogMalformed(filepath.Join(root, fanout.Name()))
			continue
		}
		fanoutPath := filepath.Join(root, fanout.Name())
		if fanout.Type()&os.ModeSymlink != 0 || !fanout.IsDir() {
			return nil, workspaceBlobIntegrity("unsafe workspace blob fanout %s", fanoutPath)
		}
		if _, err := s.ensureSafeDirectory(fanoutPath, false); err != nil {
			return nil, err
		}
		entries, err := s.fs.ReadDir(fanoutPath)
		if err != nil {
			return nil, fmt.Errorf("read workspace blob fanout directory: %w", err)
		}
		for _, entry := range entries {
			path := filepath.Join(fanoutPath, entry.Name())
			if entry.Type()&os.ModeSymlink != 0 || entry.IsDir() ||
				!workspaceBlobLowerHex(entry.Name(), 64) || !strings.HasPrefix(entry.Name(), fanout.Name()) {
				workspaceBlobLogMalformed(path)
				continue
			}
			hash, err := dto.ParseWorkspaceContentHash("blake3:" + entry.Name())
			if err != nil {
				workspaceBlobLogMalformed(path)
				continue
			}
			info, err := s.fs.Lstat(path)
			if err != nil {
				if errors.Is(err, fs.ErrNotExist) {
					continue
				}
				return nil, fmt.Errorf("stat workspace blob final: %w", err)
			}
			if !info.Mode().IsRegular() {
				workspaceBlobLogMalformed(path)
				continue
			}
			finals = append(finals, workspaceBlobFinal{hash: hash, path: path, info: info})
		}
	}
	sort.Slice(finals, func(i, j int) bool { return finals[i].hash < finals[j].hash })
	return finals, nil
}

func (s *workspaceBlobStore) canonicalFinalExists(path string) (bool, error) {
	info, err := s.fs.Lstat(path)
	if errors.Is(err, fs.ErrNotExist) {
		return false, nil
	}
	if err != nil {
		return false, fmt.Errorf("lstat workspace blob final: %w", err)
	}
	if info.Mode()&os.ModeSymlink != 0 || !info.Mode().IsRegular() {
		return false, workspaceBlobIntegrity("unsafe final type at %s", path)
	}
	return true, nil
}

func (s *workspaceBlobStore) syncDirIfPresent(path string) error {
	exists, err := s.ensureSafeDirectory(path, false)
	if err != nil || !exists {
		return err
	}
	err = s.fs.SyncDir(path)
	if errors.Is(err, fs.ErrNotExist) {
		return nil
	}
	if err != nil {
		return fmt.Errorf("sync workspace blob fanout directory: %w", err)
	}
	return nil
}

func workspaceBlobLowerHex(value string, length int) bool {
	if len(value) != length {
		return false
	}
	for i := range len(value) {
		if (value[i] < '0' || value[i] > '9') && (value[i] < 'a' || value[i] > 'f') {
			return false
		}
	}
	return true
}

func workspaceBlobZeroHash() dto.WorkspaceContentHash {
	return dto.WorkspaceContentHash("blake3:" + strings.Repeat("0", 64))
}

func workspaceBlobIntegrity(format string, args ...any) error {
	return fmt.Errorf("%w: %s", ErrWorkspaceBlobIntegrity, fmt.Sprintf(format, args...))
}

func workspaceBlobLogMalformed(path string) {
	zap.L().Warn("ignoring malformed workspace blob entry", zap.String("path", path))
}

type workspaceBlobContextReader struct {
	ctx    context.Context
	reader io.Reader
}

func (r *workspaceBlobContextReader) Read(p []byte) (int, error) {
	if err := r.ctx.Err(); err != nil {
		return 0, err
	}
	return r.reader.Read(p)
}

type workspaceUTF8Validator struct {
	valid   bool
	pending []byte
}

func newWorkspaceUTF8Validator() *workspaceUTF8Validator {
	return &workspaceUTF8Validator{valid: true, pending: make([]byte, 0, utf8.UTFMax)}
}

func (v *workspaceUTF8Validator) Write(p []byte) (int, error) {
	originalLength := len(p)
	if !v.valid {
		return originalLength, nil
	}
	if len(v.pending) != 0 {
		for len(p) != 0 && !utf8.FullRune(v.pending) {
			v.pending = append(v.pending, p[0])
			p = p[1:]
		}
		if utf8.FullRune(v.pending) {
			runeValue, size := utf8.DecodeRune(v.pending)
			if runeValue == utf8.RuneError && size == 1 {
				v.valid = false
				v.pending = v.pending[:0]
				return originalLength, nil
			}
			v.pending = v.pending[:0]
		}
	}
	for len(p) != 0 {
		if p[0] < utf8.RuneSelf {
			p = p[1:]
			continue
		}
		if !utf8.FullRune(p) {
			v.pending = append(v.pending[:0], p...)
			return originalLength, nil
		}
		runeValue, size := utf8.DecodeRune(p)
		if runeValue == utf8.RuneError && size == 1 {
			v.valid = false
			return originalLength, nil
		}
		p = p[size:]
	}
	return originalLength, nil
}

func (v *workspaceUTF8Validator) Valid() bool {
	return v.valid && len(v.pending) == 0
}

func (osWorkspaceBlobFS) CreateTemp(dir, pattern string) (workspaceBlobFile, error) {
	return os.CreateTemp(dir, pattern)
}

func (osWorkspaceBlobFS) Open(name string) (workspaceBlobFile, error) {
	return os.Open(name)
}

func (osWorkspaceBlobFS) Lstat(name string) (os.FileInfo, error) {
	return os.Lstat(name)
}

func (osWorkspaceBlobFS) MkdirAll(path string, perm fs.FileMode) error {
	return os.MkdirAll(path, perm)
}

func (osWorkspaceBlobFS) Rename(oldPath, newPath string) error {
	return os.Rename(oldPath, newPath)
}

func (osWorkspaceBlobFS) Remove(path string) error {
	return os.Remove(path)
}

func (osWorkspaceBlobFS) ReadDir(path string) ([]os.DirEntry, error) {
	return os.ReadDir(path)
}

func (osWorkspaceBlobFS) SyncDir(path string) error {
	directory, err := os.Open(path)
	if err != nil {
		return err
	}
	defer directory.Close()
	return directory.Sync()
}
