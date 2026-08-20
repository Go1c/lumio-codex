package dao

import (
	"bytes"
	"context"
	"database/sql"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"sort"
	"strconv"
	"sync"
	"time"

	"github.com/haierkeys/fast-note-sync-service/internal/domain"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/haierkeys/fast-note-sync-service/internal/model"
	"github.com/zeebo/blake3"
	"gorm.io/gorm"
	"gorm.io/gorm/clause"
)

type workspaceMigrationState struct {
	once sync.Once
	err  error
}

type workspaceRepository struct {
	dao        *Dao
	migrations sync.Map
}

type workspaceTx struct {
	db            *gorm.DB
	lockWorkspace bool
}

type workspaceReadSnapshot struct {
	*workspaceTx
	mu     sync.Mutex
	closed bool
}

func newWorkspaceRepository(dao *Dao) *workspaceRepository {
	return &workspaceRepository{dao: dao}
}

func NewWorkspaceRepository(dao *Dao) domain.WorkspaceRepository {
	return newWorkspaceRepository(dao)
}

func (r *workspaceRepository) GetKey(uid int64) string {
	return "user_workspace_" + strconv.FormatInt(uid, 10)
}

func init() {
	RegisterModel(ModelConfig{
		Name: "Workspace",
		RepoFactory: func(d *Dao) daoDBCustomKey {
			return newWorkspaceRepository(d)
		},
		IsMainDB: false,
	})
}

func (r *workspaceRepository) Migrate(ctx context.Context, uid int64) error {
	if err := workspaceValidateUID(uid); err != nil {
		return err
	}
	if err := ctx.Err(); err != nil {
		return err
	}
	key := r.GetKey(uid)
	actual, _ := r.migrations.LoadOrStore(key, &workspaceMigrationState{})
	state := actual.(*workspaceMigrationState)
	state.once.Do(func() {
		state.err = r.dao.AutoMigrate(uid, "Workspace")
	})
	return state.err
}

func (r *workspaceRepository) Read(
	ctx context.Context,
	uid int64,
	fn func(domain.WorkspaceReadTx) error,
) error {
	if err := workspaceValidateUID(uid); err != nil {
		return err
	}
	if fn == nil {
		return errors.New("workspace read callback is nil")
	}
	return r.dao.ExecuteRead(ctx, uid, r, func(db *gorm.DB) error {
		run := func(tx *gorm.DB) error {
			return fn(&workspaceTx{db: tx})
		}
		if db.Dialector.Name() == "sqlite" {
			return db.Transaction(run)
		}
		return db.Transaction(run, &sql.TxOptions{
			Isolation: sql.LevelRepeatableRead,
			ReadOnly:  true,
		})
	})
}

func (r *workspaceRepository) OpenReadSnapshot(
	ctx context.Context,
	uid int64,
) (domain.WorkspaceReadSnapshot, error) {
	if err := workspaceValidateUID(uid); err != nil {
		return nil, err
	}
	if err := ctx.Err(); err != nil {
		return nil, err
	}
	var snapshot *workspaceReadSnapshot
	err := r.dao.ExecuteRead(ctx, uid, r, func(db *gorm.DB) error {
		options := make([]*sql.TxOptions, 0, 1)
		if db.Dialector.Name() != "sqlite" {
			options = append(options, &sql.TxOptions{
				Isolation: sql.LevelRepeatableRead,
				ReadOnly:  true,
			})
		}
		begun := db.WithContext(ctx).Begin(options...)
		if begun.Error != nil {
			return begun.Error
		}
		snapshot = &workspaceReadSnapshot{
			workspaceTx: &workspaceTx{db: begun},
		}
		return nil
	})
	if err != nil {
		if snapshot != nil {
			_ = snapshot.Close()
		}
		return nil, err
	}
	return snapshot, nil
}

func (snapshot *workspaceReadSnapshot) Close() error {
	if snapshot == nil || snapshot.workspaceTx == nil || snapshot.db == nil {
		return nil
	}
	snapshot.mu.Lock()
	defer snapshot.mu.Unlock()
	if snapshot.closed {
		return nil
	}
	snapshot.closed = true
	return snapshot.db.Rollback().Error
}

func (r *workspaceRepository) Write(
	ctx context.Context,
	uid int64,
	fn func(domain.WorkspaceWriteTx) error,
) error {
	if err := workspaceValidateUID(uid); err != nil {
		return err
	}
	if fn == nil {
		return errors.New("workspace write callback is nil")
	}
	return r.dao.ExecuteWriteWithRetry(ctx, uid, r, func(db *gorm.DB) error {
		return db.Transaction(func(tx *gorm.DB) error {
			return fn(&workspaceTx{
				db:            tx,
				lockWorkspace: tx.Dialector.Name() != "sqlite",
			})
		})
	})
}

func (tx *workspaceTx) Workspace(workspaceID string) (*domain.WorkspaceRecord, error) {
	var stored model.Workspace
	query := tx.workspaceLock(tx.db).Where("workspace_id = ?", workspaceID).Take(&stored)
	if query.Error != nil {
		return nil, workspaceNotFound(query.Error)
	}
	return workspaceToDomain(&stored), nil
}

func (tx *workspaceTx) Workspaces() ([]domain.WorkspaceRecord, error) {
	var stored []model.Workspace
	if err := tx.db.Order("workspace_id ASC").Find(&stored).Error; err != nil {
		return nil, err
	}
	result := make([]domain.WorkspaceRecord, 0, len(stored))
	for i := range stored {
		result = append(result, *workspaceToDomain(&stored[i]))
	}
	return result, nil
}

func (tx *workspaceTx) WorkspacesPage(afterID int64, limit int) ([]domain.WorkspaceRecord, error) {
	if limit <= 0 {
		return nil, errors.New("workspace page limit must be positive")
	}
	var stored []model.Workspace
	if err := tx.db.Where("id > ?", afterID).Order("id ASC").Limit(limit).Find(&stored).Error; err != nil {
		return nil, err
	}
	result := make([]domain.WorkspaceRecord, 0, len(stored))
	for i := range stored {
		result = append(result, *workspaceToDomain(&stored[i]))
	}
	return result, nil
}

func (tx *workspaceTx) Client(workspaceID, clientID string) (*domain.WorkspaceClientRecord, error) {
	var stored model.WorkspaceClient
	err := tx.db.Where("workspace_id = ? AND client_id = ?", workspaceID, clientID).Take(&stored).Error
	if err != nil {
		return nil, workspaceNotFound(err)
	}
	return workspaceClientToDomain(&stored), nil
}

func (tx *workspaceTx) Path(workspaceID string, path dto.WorkspacePath) (*domain.WorkspacePathRecord, error) {
	canonical, err := dto.ParseWorkspacePath(string(path))
	if err != nil {
		return nil, err
	}
	pathKey := workspacePathKey(canonical)
	var stored model.WorkspacePath
	err = tx.db.Where("workspace_id = ? AND path_key = ?", workspaceID, pathKey).Take(&stored).Error
	if err != nil {
		return nil, workspaceNotFound(err)
	}
	if stored.Path != string(canonical) {
		return nil, workspacePathCollision(workspaceID, pathKey, string(canonical), stored.Path)
	}
	return workspacePathToDomain(&stored), nil
}

func (tx *workspaceTx) Paths(workspaceID string) ([]domain.WorkspacePathRecord, error) {
	var stored []model.WorkspacePath
	if err := tx.db.Where("workspace_id = ?", workspaceID).Order("path_key ASC").Find(&stored).Error; err != nil {
		return nil, err
	}
	result := make([]domain.WorkspacePathRecord, 0, len(stored))
	for i := range stored {
		result = append(result, *workspacePathToDomain(&stored[i]))
	}
	return result, nil
}

func (tx *workspaceTx) EventsAfter(
	workspaceID string,
	after, through dto.WorkspaceRevision,
) ([]domain.WorkspaceEventRecord, error) {
	var stored []model.WorkspaceEvent
	err := tx.db.
		Where("workspace_id = ? AND revision > ? AND revision <= ?", workspaceID, uint64(after), uint64(through)).
		Order("revision ASC").
		Find(&stored).Error
	if err != nil {
		return nil, err
	}
	result := make([]domain.WorkspaceEventRecord, 0, len(stored))
	for i := range stored {
		result = append(result, *workspaceEventToDomain(&stored[i]))
	}
	return result, nil
}

func (tx *workspaceTx) PrunableEvents(
	workspaceID string,
	olderThan time.Time,
	atOrBefore dto.WorkspaceRevision,
	limit int,
) ([]domain.WorkspaceEventRecord, error) {
	if limit <= 0 {
		return nil, errors.New("workspace event page limit must be positive")
	}
	var stored []model.WorkspaceEvent
	err := tx.db.Where(
		"workspace_id = ? AND (created_at < ? OR revision <= ?)",
		workspaceID, olderThan, uint64(atOrBefore),
	).Order("revision ASC").Limit(limit).Find(&stored).Error
	if err != nil {
		return nil, err
	}
	result := make([]domain.WorkspaceEventRecord, 0, len(stored))
	for i := range stored {
		result = append(result, *workspaceEventToDomain(&stored[i]))
	}
	return result, nil
}

func (tx *workspaceTx) Operation(clientID, operationID string) (*domain.WorkspaceOperationRecord, error) {
	var stored model.WorkspaceOperation
	err := tx.db.Where("client_id = ? AND operation_id = ?", clientID, operationID).Take(&stored).Error
	if err != nil {
		return nil, workspaceNotFound(err)
	}
	return workspaceOperationToDomain(&stored), nil
}

func (tx *workspaceTx) ExpiredWaitingOperations(
	workspaceID string, before time.Time, limit int,
) ([]domain.WorkspaceOperationRecord, error) {
	if limit <= 0 {
		return nil, errors.New("workspace operation page limit must be positive")
	}
	var stored []model.WorkspaceOperation
	err := tx.db.Where(
		"workspace_id = ? AND state = ? AND expires_at IS NOT NULL AND expires_at <= ?",
		workspaceID, "waiting_blob", before,
	).Order("expires_at ASC, client_id ASC, operation_id ASC").Limit(limit).Find(&stored).Error
	if err != nil {
		return nil, err
	}
	result := make([]domain.WorkspaceOperationRecord, 0, len(stored))
	for i := range stored {
		result = append(result, *workspaceOperationToDomain(&stored[i]))
	}
	return result, nil
}

func (tx *workspaceTx) Conflict(workspaceID, conflictID string) (*domain.WorkspaceConflictRecord, error) {
	var stored model.WorkspaceConflict
	err := tx.db.Where("workspace_id = ? AND conflict_id = ?", workspaceID, conflictID).Take(&stored).Error
	if err != nil {
		return nil, workspaceNotFound(err)
	}
	return workspaceConflictToDomain(&stored), nil
}

func (tx *workspaceTx) PendingConflict(workspaceID string, path dto.WorkspacePath) (*domain.WorkspaceConflictRecord, error) {
	canonical, err := dto.ParseWorkspacePath(string(path))
	if err != nil {
		return nil, err
	}
	var stored model.WorkspaceConflict
	err = tx.db.Where(
		"workspace_id = ? AND path_key = ? AND path = ? AND status = ?",
		workspaceID, workspacePathKey(canonical), string(canonical), "pending",
	).Order("updated_at DESC, conflict_id DESC").Take(&stored).Error
	if err != nil {
		return nil, workspaceNotFound(err)
	}
	return workspaceConflictToDomain(&stored), nil
}

func (tx *workspaceTx) PendingConflicts(workspaceID string) ([]domain.WorkspaceConflictRecord, error) {
	var stored []model.WorkspaceConflict
	if err := tx.db.Where("workspace_id = ? AND status = ?", workspaceID, "pending").
		Order("conflict_id ASC").Find(&stored).Error; err != nil {
		return nil, err
	}
	result := make([]domain.WorkspaceConflictRecord, 0, len(stored))
	for i := range stored {
		result = append(result, *workspaceConflictToDomain(&stored[i]))
	}
	return result, nil
}

func (tx *workspaceTx) PendingConflictCount(workspaceID string) (int64, error) {
	var count int64
	err := tx.db.Model(&model.WorkspaceConflict{}).
		Where("workspace_id = ? AND status = ?", workspaceID, "pending").
		Count(&count).Error
	return count, err
}

func (tx *workspaceTx) PendingConflictBoundary(workspaceID string) (string, error) {
	var stored model.WorkspaceConflict
	err := tx.db.Where("workspace_id = ? AND status = ?", workspaceID, "pending").
		Order("conflict_id DESC").Take(&stored).Error
	if errors.Is(err, gorm.ErrRecordNotFound) {
		return "", nil
	}
	if err != nil {
		return "", err
	}
	return stored.ConflictID, nil
}

func (tx *workspaceTx) PendingConflictPage(
	workspaceID, afterID, throughID string, limit int,
) ([]domain.WorkspaceConflictRecord, error) {
	if limit <= 0 {
		return nil, errors.New("workspace conflict page limit must be positive")
	}
	query := tx.db.Where("workspace_id = ? AND status = ?", workspaceID, "pending")
	if afterID != "" {
		query = query.Where("conflict_id > ?", afterID)
	}
	if throughID != "" {
		query = query.Where("conflict_id <= ?", throughID)
	}
	var stored []model.WorkspaceConflict
	if err := query.Order("conflict_id ASC").Limit(limit).Find(&stored).Error; err != nil {
		return nil, err
	}
	result := make([]domain.WorkspaceConflictRecord, 0, len(stored))
	for i := range stored {
		result = append(result, *workspaceConflictToDomain(&stored[i]))
	}
	return result, nil
}

func (tx *workspaceTx) Operations(workspaceID string) ([]domain.WorkspaceOperationRecord, error) {
	var stored []model.WorkspaceOperation
	if err := tx.db.Where("workspace_id = ?", workspaceID).
		Order("created_at ASC, client_id ASC, operation_id ASC").Find(&stored).Error; err != nil {
		return nil, err
	}
	result := make([]domain.WorkspaceOperationRecord, 0, len(stored))
	for i := range stored {
		result = append(result, *workspaceOperationToDomain(&stored[i]))
	}
	return result, nil
}

func (tx *workspaceTx) Clients(workspaceID string) ([]domain.WorkspaceClientRecord, error) {
	var stored []model.WorkspaceClient
	if err := tx.db.Where("workspace_id = ?", workspaceID).
		Order("client_id ASC").Find(&stored).Error; err != nil {
		return nil, err
	}
	result := make([]domain.WorkspaceClientRecord, 0, len(stored))
	for i := range stored {
		result = append(result, *workspaceClientToDomain(&stored[i]))
	}
	return result, nil
}

func (tx *workspaceTx) ClientsPage(
	workspaceID, afterClientID string, limit int,
) ([]domain.WorkspaceClientRecord, error) {
	if limit <= 0 {
		return nil, errors.New("workspace client page limit must be positive")
	}
	query := tx.db.Where("workspace_id = ?", workspaceID)
	if afterClientID != "" {
		query = query.Where("client_id > ?", afterClientID)
	}
	var stored []model.WorkspaceClient
	if err := query.Order("client_id ASC").Limit(limit).Find(&stored).Error; err != nil {
		return nil, err
	}
	result := make([]domain.WorkspaceClientRecord, 0, len(stored))
	for i := range stored {
		result = append(result, *workspaceClientToDomain(&stored[i]))
	}
	return result, nil
}

func (tx *workspaceTx) Tombstones(
	workspaceID string, through dto.WorkspaceRevision, limit int,
) ([]domain.WorkspacePathRecord, error) {
	if limit <= 0 {
		return nil, errors.New("workspace tombstone page limit must be positive")
	}
	var stored []model.WorkspacePath
	if err := tx.db.Where(
		"workspace_id = ? AND tombstone = ? AND path_revision <= ?",
		workspaceID, true, uint64(through),
	).Order("path_revision ASC, path_key ASC").Limit(limit).Find(&stored).Error; err != nil {
		return nil, err
	}
	result := make([]domain.WorkspacePathRecord, 0, len(stored))
	for i := range stored {
		result = append(result, *workspacePathToDomain(&stored[i]))
	}
	return result, nil
}

func (tx *workspaceTx) Blob(hash dto.WorkspaceContentHash) (*domain.WorkspaceBlobRecord, error) {
	canonical, err := dto.ParseWorkspaceContentHash(string(hash))
	if err != nil {
		return nil, err
	}
	var stored model.WorkspaceBlob
	err = tx.workspaceLock(tx.db).Where("content_hash = ?", string(canonical)).Take(&stored).Error
	if err != nil {
		return nil, workspaceNotFound(err)
	}
	return workspaceBlobToDomain(&stored), nil
}

func (tx *workspaceTx) BlobsAfter(
	after *dto.WorkspaceContentHash,
	limit int,
) ([]domain.WorkspaceBlobRecord, error) {
	if limit <= 0 {
		return nil, fmt.Errorf("workspace blob page limit must be positive")
	}
	query := tx.db.Order("content_hash ASC").Limit(limit)
	if after != nil {
		canonical, err := dto.ParseWorkspaceContentHash(string(*after))
		if err != nil {
			return nil, err
		}
		query = query.Where("content_hash > ?", string(canonical))
	}
	var stored []model.WorkspaceBlob
	if err := query.Find(&stored).Error; err != nil {
		return nil, err
	}
	result := make([]domain.WorkspaceBlobRecord, 0, len(stored))
	for i := range stored {
		result = append(result, *workspaceBlobToDomain(&stored[i]))
	}
	return result, nil
}

func (tx *workspaceTx) workspaceLock(db *gorm.DB) *gorm.DB {
	if !tx.lockWorkspace {
		return db
	}
	return db.Clauses(clause.Locking{Strength: clause.LockingStrengthUpdate})
}

func workspacePathKey(path dto.WorkspacePath) string {
	sum := blake3.Sum256([]byte(path))
	return hex.EncodeToString(sum[:])
}

func workspacePathCollision(
	workspaceID, pathKey, requestedPath, storedPath string,
) *domain.WorkspacePathKeyCollisionError {
	return &domain.WorkspacePathKeyCollisionError{
		WorkspaceID:   workspaceID,
		PathKey:       pathKey,
		RequestedPath: requestedPath,
		StoredPath:    storedPath,
	}
}

func workspaceNotFound(err error) error {
	if errors.Is(err, gorm.ErrRecordNotFound) {
		return domain.ErrWorkspaceRecordNotFound
	}
	return err
}

func workspaceUniqueError(db *gorm.DB, err error, entity, key string) error {
	if err == nil {
		return nil
	}
	translated := err
	if translator, ok := db.Dialector.(gorm.ErrorTranslator); ok {
		translated = translator.Translate(err)
	}
	if errors.Is(translated, gorm.ErrDuplicatedKey) {
		return &domain.WorkspaceUniqueConstraintError{Entity: entity, Key: key}
	}
	return err
}

func workspaceToDomain(stored *model.Workspace) *domain.WorkspaceRecord {
	return &domain.WorkspaceRecord{
		ID:                  stored.ID,
		WorkspaceID:         stored.WorkspaceID,
		GlobalRevision:      dto.WorkspaceRevision(stored.GlobalRevision),
		ReplayFloorRevision: dto.WorkspaceRevision(stored.ReplayFloorRevision),
		LivePathCount:       stored.LivePathCount,
		LiveBytes:           stored.LiveBytes,
		CreatedAt:           stored.CreatedAt,
		UpdatedAt:           stored.UpdatedAt,
	}
}

func workspaceClientToDomain(stored *model.WorkspaceClient) *domain.WorkspaceClientRecord {
	return &domain.WorkspaceClientRecord{
		WorkspaceID:     stored.WorkspaceID,
		ClientID:        stored.ClientID,
		LastAckRevision: dto.WorkspaceRevision(stored.LastAckRevision),
		LastSeenAt:      stored.LastSeenAt,
	}
}

func workspacePathToDomain(stored *model.WorkspacePath) *domain.WorkspacePathRecord {
	return &domain.WorkspacePathRecord{
		ID:           stored.ID,
		WorkspaceID:  stored.WorkspaceID,
		Path:         dto.WorkspacePath(stored.Path),
		PathRevision: dto.WorkspaceRevision(stored.PathRevision),
		Kind:         dto.WorkspaceEntryKind(stored.Kind),
		ContentHash:  workspaceHashToDomain(stored.ContentHash),
		Size:         stored.Size,
		ModifiedAtMS: stored.ModifiedAtMS,
		Executable:   stored.Executable,
		Tombstone:    stored.Tombstone,
	}
}

func workspaceEventToDomain(stored *model.WorkspaceEvent) *domain.WorkspaceEventRecord {
	return &domain.WorkspaceEventRecord{
		ID:               stored.ID,
		WorkspaceID:      stored.WorkspaceID,
		Revision:         dto.WorkspaceRevision(stored.Revision),
		Kind:             stored.Kind,
		OperationID:      stored.OperationID,
		OriginClientID:   stored.OriginClientID,
		MutationJSON:     workspaceBytes(stored.MutationJSON),
		PathStateJSON:    workspaceBytes(stored.PathStateJSON),
		OldPathStateJSON: workspaceBytes(stored.OldPathStateJSON),
		NewPathStateJSON: workspaceBytes(stored.NewPathStateJSON),
		ResolvedJSON:     workspaceBytes(stored.ResolvedJSON),
		CreatedAt:        stored.CreatedAt,
	}
}

func workspaceOperationToDomain(stored *model.WorkspaceOperation) *domain.WorkspaceOperationRecord {
	return &domain.WorkspaceOperationRecord{
		WorkspaceID:      stored.WorkspaceID,
		ClientID:         stored.ClientID,
		OperationID:      stored.OperationID,
		RequestKind:      stored.RequestKind,
		RequestDigest:    stored.RequestDigest,
		State:            stored.State,
		ResultAction:     workspaceString(stored.ResultAction),
		ResultJSON:       workspaceBytes(stored.ResultJSON),
		ConflictJSON:     workspaceBytes(stored.ConflictJSON),
		RequiredHash:     workspaceHashToDomain(stored.RequiredHash),
		ConflictRevision: workspaceConflictRevisionToDomain(stored.ConflictRevision),
		ExpiresAt:        workspaceTime(stored.ExpiresAt),
		CreatedAt:        stored.CreatedAt,
		UpdatedAt:        stored.UpdatedAt,
	}
}

func workspaceConflictToDomain(stored *model.WorkspaceConflict) *domain.WorkspaceConflictRecord {
	return &domain.WorkspaceConflictRecord{
		WorkspaceID:             stored.WorkspaceID,
		ConflictID:              stored.ConflictID,
		ConflictRevision:        workspaceConflictRevisionValue(stored.ConflictRevision),
		RenameTargetJSON:        workspaceBytes(stored.RenameTargetJSON),
		Path:                    dto.WorkspacePath(stored.Path),
		Kind:                    dto.WorkspaceConflictKind(stored.Kind),
		Status:                  stored.Status,
		AncestorJSON:            workspaceBytes(stored.AncestorJSON),
		CurrentJSON:             workspaceBytes(stored.CurrentJSON),
		IncomingJSON:            workspaceBytes(stored.IncomingJSON),
		CreatedByOperationID:    stored.CreatedByOperationID,
		ResolutionOperationID:   workspaceString(stored.ResolutionOperationID),
		ResolutionRevision:      workspaceRevisionToDomain(stored.ResolutionRevision),
		ResolutionChoice:        workspaceChoiceToDomain(stored.ResolutionChoice),
		ResolutionPathStateJSON: workspaceBytes(stored.ResolutionPathStateJSON),
		ResolvedByClientID:      workspaceString(stored.ResolvedByClientID),
		ResolvedAt:              workspaceTime(stored.ResolvedAt),
		CreatedAt:               stored.CreatedAt,
		UpdatedAt:               stored.UpdatedAt,
	}
}

func workspaceBlobToDomain(stored *model.WorkspaceBlob) *domain.WorkspaceBlobRecord {
	return &domain.WorkspaceBlobRecord{
		ContentHash:    dto.WorkspaceContentHash(stored.ContentHash),
		Size:           stored.Size,
		UTF8Valid:      stored.UTF8Valid,
		RefCount:       stored.RefCount,
		UnreferencedAt: workspaceTime(stored.UnreferencedAt),
		CreatedAt:      stored.CreatedAt,
		UpdatedAt:      stored.UpdatedAt,
	}
}

func workspaceBytes(value []byte) []byte {
	return append([]byte(nil), value...)
}

func workspaceString(value *string) *string {
	if value == nil {
		return nil
	}
	copy := *value
	return &copy
}

func workspaceTime(value *time.Time) *time.Time {
	if value == nil {
		return nil
	}
	copy := *value
	return &copy
}

func workspaceHashToDomain(value *string) *dto.WorkspaceContentHash {
	if value == nil {
		return nil
	}
	hash := dto.WorkspaceContentHash(*value)
	return &hash
}

func workspaceRevisionToDomain(value *uint64) *dto.WorkspaceRevision {
	if value == nil {
		return nil
	}
	revision := dto.WorkspaceRevision(*value)
	return &revision
}

func workspaceConflictRevisionToDomain(value *string) *dto.WorkspaceConflictRevision {
	if value == nil {
		return nil
	}
	revision, err := dto.ParseWorkspaceConflictRevision(*value)
	if err != nil {
		return nil
	}
	return &revision
}

func workspaceConflictRevisionValue(value string) dto.WorkspaceConflictRevision {
	revision, err := dto.ParseWorkspaceConflictRevision(value)
	if err != nil {
		return dto.WorkspaceConflictRevision{}
	}
	return revision
}

func workspaceChoiceToDomain(value *string) *dto.WorkspaceConflictChoice {
	if value == nil {
		return nil
	}
	choice := dto.WorkspaceConflictChoice(*value)
	return &choice
}

func workspaceHashToModel(value *dto.WorkspaceContentHash) *string {
	if value == nil {
		return nil
	}
	hash := string(*value)
	return &hash
}

func workspaceRevisionToModel(value *dto.WorkspaceRevision) *uint64 {
	if value == nil {
		return nil
	}
	revision := uint64(*value)
	return &revision
}

func workspaceConflictRevisionToModel(value *dto.WorkspaceConflictRevision) *string {
	if value == nil {
		return nil
	}
	encoded, err := json.Marshal(value)
	if err != nil {
		return nil
	}
	var result string
	if err := json.Unmarshal(encoded, &result); err != nil {
		return nil
	}
	return &result
}

func workspaceConflictRevisionString(value dto.WorkspaceConflictRevision) string {
	encoded := workspaceConflictRevisionToModel(&value)
	if encoded == nil {
		return ""
	}
	return *encoded
}

func workspaceChoiceToModel(value *dto.WorkspaceConflictChoice) *string {
	if value == nil {
		return nil
	}
	choice := string(*value)
	return &choice
}

func workspaceValidateHash(value dto.WorkspaceContentHash) (dto.WorkspaceContentHash, error) {
	return dto.ParseWorkspaceContentHash(string(value))
}

func workspaceValidateOptionalHash(value *dto.WorkspaceContentHash) error {
	if value == nil {
		return nil
	}
	_, err := workspaceValidateHash(*value)
	return err
}

func workspaceValidateUID(uid int64) error {
	if uid <= 0 {
		return domain.ErrWorkspaceInvalidUID
	}
	return nil
}

func (tx *workspaceTx) CreateWorkspace(record domain.WorkspaceRecord) error {
	if record.LivePathCount < 0 {
		return &domain.WorkspaceCounterUnderflowError{Counter: "live_path_count", Value: record.LivePathCount}
	}
	var count int64
	if err := tx.db.Model(&model.Workspace{}).
		Where("workspace_id = ?", record.WorkspaceID).
		Count(&count).Error; err != nil {
		return err
	}
	if count != 0 {
		return &domain.WorkspaceUniqueConstraintError{Entity: "workspace", Key: record.WorkspaceID}
	}
	stored := workspaceModel(record)
	return workspaceUniqueError(tx.db, tx.db.Create(&stored).Error, "workspace", record.WorkspaceID)
}

func (tx *workspaceTx) SaveWorkspace(record domain.WorkspaceRecord) error {
	if record.LivePathCount < 0 {
		return &domain.WorkspaceCounterUnderflowError{Counter: "live_path_count", Value: record.LivePathCount}
	}
	var stored model.Workspace
	err := tx.workspaceLock(tx.db).Where("workspace_id = ?", record.WorkspaceID).Take(&stored).Error
	if err != nil {
		return workspaceNotFound(err)
	}
	updatedAt := record.UpdatedAt
	if updatedAt.IsZero() {
		updatedAt = time.Now().UTC()
	}
	return tx.db.Model(&model.Workspace{}).Where("id = ?", stored.ID).Updates(map[string]any{
		"global_revision":       uint64(record.GlobalRevision),
		"replay_floor_revision": uint64(record.ReplayFloorRevision),
		"live_path_count":       record.LivePathCount,
		"live_bytes":            record.LiveBytes,
		"updated_at":            updatedAt,
	}).Error
}

func (tx *workspaceTx) SaveClient(record domain.WorkspaceClientRecord) error {
	var stored model.WorkspaceClient
	err := tx.db.Where("workspace_id = ? AND client_id = ?", record.WorkspaceID, record.ClientID).
		Take(&stored).Error
	if errors.Is(err, gorm.ErrRecordNotFound) {
		created := model.WorkspaceClient{
			WorkspaceID:     record.WorkspaceID,
			ClientID:        record.ClientID,
			LastAckRevision: uint64(record.LastAckRevision),
			LastSeenAt:      record.LastSeenAt,
		}
		key := record.WorkspaceID + "/" + record.ClientID
		return workspaceUniqueError(tx.db, tx.db.Create(&created).Error, "client", key)
	}
	if err != nil {
		return err
	}
	return tx.db.Model(&model.WorkspaceClient{}).
		Where("workspace_id = ? AND client_id = ?", record.WorkspaceID, record.ClientID).
		Updates(map[string]any{
			"last_ack_revision": uint64(record.LastAckRevision),
			"last_seen_at":      record.LastSeenAt,
		}).Error
}

func (tx *workspaceTx) SavePath(record domain.WorkspacePathRecord) error {
	canonical, err := dto.ParseWorkspacePath(string(record.Path))
	if err != nil {
		return err
	}
	if err := workspaceValidateOptionalHash(record.ContentHash); err != nil {
		return err
	}
	pathKey := workspacePathKey(canonical)
	var existing model.WorkspacePath
	err = tx.db.Where("workspace_id = ? AND path_key = ?", record.WorkspaceID, pathKey).
		Take(&existing).Error
	switch {
	case err == nil:
		if existing.Path != string(canonical) {
			return workspacePathCollision(record.WorkspaceID, pathKey, string(canonical), existing.Path)
		}
		if record.ID != 0 && record.ID != existing.ID {
			return &domain.WorkspaceUniqueConstraintError{
				Entity: "path",
				Key:    record.WorkspaceID + "/" + pathKey,
			}
		}
	case errors.Is(err, gorm.ErrRecordNotFound):
		if record.ID != 0 {
			err = tx.db.Where("id = ?", record.ID).Take(&existing).Error
			if err != nil {
				return workspaceNotFound(err)
			}
			if existing.WorkspaceID != record.WorkspaceID {
				return &domain.WorkspaceUniqueConstraintError{
					Entity: "path_id",
					Key:    strconv.FormatInt(record.ID, 10),
				}
			}
		}
	default:
		return err
	}

	stored := workspacePathModel(record, canonical, pathKey)
	if existing.ID == 0 {
		stored.ID = 0
		key := record.WorkspaceID + "/" + pathKey
		return workspaceUniqueError(tx.db, tx.db.Create(&stored).Error, "path", key)
	}
	stored.ID = existing.ID
	return workspaceUniqueError(tx.db, tx.db.Model(&model.WorkspacePath{}).
		Where("id = ?", existing.ID).
		Updates(map[string]any{
			"workspace_id":   stored.WorkspaceID,
			"path_key":       stored.PathKey,
			"path":           stored.Path,
			"path_revision":  stored.PathRevision,
			"kind":           stored.Kind,
			"content_hash":   stored.ContentHash,
			"size":           stored.Size,
			"modified_at_ms": stored.ModifiedAtMS,
			"executable":     stored.Executable,
			"tombstone":      stored.Tombstone,
		}).Error, "path", record.WorkspaceID+"/"+pathKey)
}

func (tx *workspaceTx) DeletePath(recordID int64) error {
	if recordID <= 0 {
		return domain.ErrWorkspaceRecordNotFound
	}
	result := tx.db.Where("id = ?", recordID).Delete(&model.WorkspacePath{})
	if result.Error != nil {
		return result.Error
	}
	if result.RowsAffected == 0 {
		return domain.ErrWorkspaceRecordNotFound
	}
	return nil
}

func (tx *workspaceTx) SaveEvent(record domain.WorkspaceEventRecord) error {
	var count int64
	if err := tx.db.Model(&model.WorkspaceEvent{}).
		Where("workspace_id = ? AND revision = ?", record.WorkspaceID, uint64(record.Revision)).
		Count(&count).Error; err != nil {
		return err
	}
	key := fmt.Sprintf("%s/%d", record.WorkspaceID, record.Revision)
	if count != 0 {
		return &domain.WorkspaceUniqueConstraintError{Entity: "event", Key: key}
	}
	kind := record.Kind
	if kind == "" {
		kind = "event"
	}
	mutationJSON := workspaceBytes(record.MutationJSON)
	if mutationJSON == nil {
		mutationJSON = []byte{}
	}
	pathStateJSON := workspaceBytes(record.PathStateJSON)
	if pathStateJSON == nil {
		pathStateJSON = []byte{}
	}
	stored := model.WorkspaceEvent{
		WorkspaceID:      record.WorkspaceID,
		Revision:         uint64(record.Revision),
		Kind:             kind,
		OperationID:      record.OperationID,
		OriginClientID:   record.OriginClientID,
		MutationJSON:     mutationJSON,
		PathStateJSON:    pathStateJSON,
		OldPathStateJSON: workspaceBytes(record.OldPathStateJSON),
		NewPathStateJSON: workspaceBytes(record.NewPathStateJSON),
		ResolvedJSON:     workspaceBytes(record.ResolvedJSON),
		CreatedAt:        record.CreatedAt,
	}
	return workspaceUniqueError(tx.db, tx.db.Create(&stored).Error, "event", key)
}

func (tx *workspaceTx) DeleteEvents(ids []int64) error {
	if len(ids) == 0 {
		return nil
	}
	return tx.db.Where("id IN ?", ids).Delete(&model.WorkspaceEvent{}).Error
}

func (tx *workspaceTx) SaveOperation(record domain.WorkspaceOperationRecord) error {
	if err := workspaceValidateOptionalHash(record.RequiredHash); err != nil {
		return err
	}
	if record.ConflictRevision != nil {
		if _, err := record.ConflictRevision.MarshalJSON(); err != nil {
			return err
		}
	}
	var existing model.WorkspaceOperation
	err := tx.workspaceLock(tx.db).Where("client_id = ? AND operation_id = ?", record.ClientID, record.OperationID).
		Take(&existing).Error
	stored := workspaceOperationModel(record)
	key := record.ClientID + "/" + record.OperationID
	if errors.Is(err, gorm.ErrRecordNotFound) {
		return workspaceUniqueError(tx.db, tx.db.Create(&stored).Error, "operation", key)
	}
	if err != nil {
		return err
	}
	if existing.State != "waiting_blob" ||
		existing.WorkspaceID != record.WorkspaceID ||
		existing.RequestKind != record.RequestKind ||
		existing.RequestDigest != record.RequestDigest {
		return workspaceOperationImmutable(existing)
	}
	switch record.State {
	case "waiting_blob":
		if existing.RequiredHash == nil || stored.RequiredHash == nil ||
			*existing.RequiredHash != *stored.RequiredHash ||
			!workspaceStringPointerEqual(existing.ConflictRevision, stored.ConflictRevision) ||
			!workspaceTimePointerEqual(existing.ExpiresAt, stored.ExpiresAt) ||
			stored.ResultAction != nil || len(stored.ResultJSON) != 0 || len(stored.ConflictJSON) != 0 {
			return workspaceOperationImmutable(existing)
		}
	case "terminal", "expired_guard":
	default:
		return workspaceOperationImmutable(existing)
	}
	updatedAt := record.UpdatedAt
	if updatedAt.IsZero() {
		updatedAt = time.Now().UTC()
	}
	result := tx.db.Model(&model.WorkspaceOperation{}).
		Where(
			"client_id = ? AND operation_id = ? AND workspace_id = ? AND request_kind = ? AND request_digest = ? AND state = ?",
			record.ClientID,
			record.OperationID,
			existing.WorkspaceID,
			existing.RequestKind,
			existing.RequestDigest,
			"waiting_blob",
		).
		Updates(map[string]any{
			"workspace_id":      stored.WorkspaceID,
			"request_kind":      stored.RequestKind,
			"request_digest":    stored.RequestDigest,
			"state":             stored.State,
			"result_action":     stored.ResultAction,
			"result_json":       stored.ResultJSON,
			"conflict_json":     stored.ConflictJSON,
			"required_hash":     stored.RequiredHash,
			"conflict_revision": stored.ConflictRevision,
			"expires_at":        stored.ExpiresAt,
			"created_at":        existing.CreatedAt,
			"updated_at":        updatedAt,
		})
	if result.Error != nil {
		return result.Error
	}
	if result.RowsAffected != 1 {
		return workspaceOperationImmutable(existing)
	}
	return nil
}

func workspaceStringPointerEqual(left, right *string) bool {
	return (left == nil && right == nil) || (left != nil && right != nil && *left == *right)
}

func workspaceTimePointerEqual(left, right *time.Time) bool {
	return (left == nil && right == nil) || (left != nil && right != nil && left.Equal(*right))
}

func workspaceOperationImmutable(existing model.WorkspaceOperation) *domain.WorkspaceOperationImmutableError {
	return &domain.WorkspaceOperationImmutableError{
		ClientID:    existing.ClientID,
		OperationID: existing.OperationID,
		State:       existing.State,
	}
}

func (tx *workspaceTx) SaveConflict(record domain.WorkspaceConflictRecord) error {
	if _, err := record.ConflictRevision.MarshalJSON(); err != nil {
		return err
	}
	canonical, err := dto.ParseWorkspacePath(string(record.Path))
	if err != nil {
		return err
	}
	var existing model.WorkspaceConflict
	err = tx.db.Where("conflict_id = ?", record.ConflictID).Take(&existing).Error
	stored := workspaceConflictModel(record, canonical)
	if errors.Is(err, gorm.ErrRecordNotFound) {
		return workspaceUniqueError(tx.db, tx.db.Create(&stored).Error, "conflict", record.ConflictID)
	}
	if err != nil {
		return err
	}
	if existing.WorkspaceID != record.WorkspaceID {
		return &domain.WorkspaceUniqueConstraintError{Entity: "conflict", Key: record.ConflictID}
	}
	if existing.Status == "pending" && record.Status == "pending" {
		if stored.ResolutionOperationID != nil || stored.ResolutionRevision != nil ||
			stored.ResolutionChoice != nil || len(stored.ResolutionPathStateJSON) != 0 ||
			stored.ResolvedByClientID != nil || stored.ResolvedAt != nil {
			return &domain.WorkspaceConflictImmutableError{ConflictID: existing.ConflictID, Status: existing.Status}
		}
		return tx.updatePendingConflict(existing, stored, record.UpdatedAt)
	}
	if existing.Status != "pending" || record.Status != "resolved" ||
		existing.ConflictRevision != stored.ConflictRevision ||
		existing.PathKey != stored.PathKey || existing.Path != stored.Path ||
		existing.Kind != stored.Kind ||
		!bytes.Equal(existing.AncestorJSON, stored.AncestorJSON) ||
		!bytes.Equal(existing.CurrentJSON, stored.CurrentJSON) ||
		!bytes.Equal(existing.IncomingJSON, stored.IncomingJSON) ||
		!bytes.Equal(existing.RenameTargetJSON, stored.RenameTargetJSON) ||
		existing.CreatedByOperationID != stored.CreatedByOperationID ||
		stored.ResolutionOperationID == nil || stored.ResolutionRevision == nil ||
		stored.ResolutionChoice == nil || len(stored.ResolutionPathStateJSON) == 0 ||
		stored.ResolvedByClientID == nil || stored.ResolvedAt == nil {
		return &domain.WorkspaceConflictImmutableError{
			ConflictID: existing.ConflictID,
			Status:     existing.Status,
		}
	}
	updatedAt := record.UpdatedAt
	if updatedAt.IsZero() {
		updatedAt = time.Now().UTC()
	}
	result := tx.db.Model(&model.WorkspaceConflict{}).
		Where("conflict_id = ? AND status = ? AND conflict_revision = ?", record.ConflictID, "pending", existing.ConflictRevision).
		Updates(map[string]any{
			"workspace_id":               stored.WorkspaceID,
			"conflict_revision":          stored.ConflictRevision,
			"path_key":                   stored.PathKey,
			"path":                       stored.Path,
			"kind":                       stored.Kind,
			"status":                     stored.Status,
			"ancestor_json":              stored.AncestorJSON,
			"current_json":               stored.CurrentJSON,
			"incoming_json":              stored.IncomingJSON,
			"rename_target_json":         stored.RenameTargetJSON,
			"created_by_operation_id":    stored.CreatedByOperationID,
			"resolution_operation_id":    stored.ResolutionOperationID,
			"resolution_revision":        stored.ResolutionRevision,
			"resolution_choice":          stored.ResolutionChoice,
			"resolution_path_state_json": stored.ResolutionPathStateJSON,
			"resolved_by_client_id":      stored.ResolvedByClientID,
			"resolved_at":                stored.ResolvedAt,
			"created_at":                 existing.CreatedAt,
			"updated_at":                 updatedAt,
		})
	if result.Error != nil {
		return result.Error
	}
	if result.RowsAffected != 1 {
		return &domain.WorkspaceConflictImmutableError{
			ConflictID: existing.ConflictID,
			Status:     existing.Status,
		}
	}
	return nil
}

func (tx *workspaceTx) updatePendingConflict(existing, stored model.WorkspaceConflict, updatedAt time.Time) error {
	if updatedAt.IsZero() {
		updatedAt = time.Now().UTC()
	}
	result := tx.db.Model(&model.WorkspaceConflict{}).
		Where("conflict_id = ? AND status = ? AND conflict_revision = ?", existing.ConflictID, "pending", existing.ConflictRevision).
		Updates(map[string]any{
			"workspace_id":               stored.WorkspaceID,
			"conflict_revision":          stored.ConflictRevision,
			"path_key":                   stored.PathKey,
			"path":                       stored.Path,
			"kind":                       stored.Kind,
			"status":                     stored.Status,
			"ancestor_json":              stored.AncestorJSON,
			"current_json":               stored.CurrentJSON,
			"incoming_json":              stored.IncomingJSON,
			"rename_target_json":         stored.RenameTargetJSON,
			"created_by_operation_id":    stored.CreatedByOperationID,
			"resolution_operation_id":    nil,
			"resolution_revision":        nil,
			"resolution_choice":          nil,
			"resolution_path_state_json": nil,
			"resolved_by_client_id":      nil,
			"resolved_at":                nil,
			"created_at":                 existing.CreatedAt,
			"updated_at":                 updatedAt,
		})
	if result.Error != nil {
		return result.Error
	}
	if result.RowsAffected != 1 {
		return &domain.WorkspaceConflictImmutableError{ConflictID: existing.ConflictID, Status: existing.Status}
	}
	return nil
}

func (tx *workspaceTx) SaveBlob(record domain.WorkspaceBlobRecord) error {
	canonical, err := workspaceValidateHash(record.ContentHash)
	if err != nil {
		return err
	}
	if record.RefCount < 0 {
		return &domain.WorkspaceCounterUnderflowError{Counter: "ref_count", Value: record.RefCount}
	}
	record.ContentHash = canonical
	var existing model.WorkspaceBlob
	err = tx.db.Where("content_hash = ?", string(canonical)).Take(&existing).Error
	stored := workspaceBlobModel(record)
	if errors.Is(err, gorm.ErrRecordNotFound) {
		return workspaceUniqueError(tx.db, tx.db.Create(&stored).Error, "blob", string(canonical))
	}
	if err != nil {
		return err
	}
	createdAt := record.CreatedAt
	if createdAt.IsZero() {
		createdAt = existing.CreatedAt
	}
	updatedAt := record.UpdatedAt
	if updatedAt.IsZero() {
		updatedAt = time.Now().UTC()
	}
	return tx.db.Model(&model.WorkspaceBlob{}).Where("content_hash = ?", string(canonical)).
		Updates(map[string]any{
			"size":            record.Size,
			"utf8_valid":      record.UTF8Valid,
			"ref_count":       record.RefCount,
			"unreferenced_at": record.UnreferencedAt,
			"created_at":      createdAt,
			"updated_at":      updatedAt,
		}).Error
}

func (tx *workspaceTx) AddBlobRef(ref domain.WorkspaceBlobRefRecord, now time.Time) error {
	canonical, err := workspaceValidateHash(ref.ContentHash)
	if err != nil {
		return err
	}
	if err := workspaceValidateBlobRefOwner(ref.OwnerType, ref.OwnerKey); err != nil {
		return err
	}
	created := model.WorkspaceBlobRef{
		ContentHash: string(canonical),
		OwnerType:   ref.OwnerType,
		OwnerKey:    ref.OwnerKey,
		CreatedAt:   now,
		UpdatedAt:   now,
	}
	insert := tx.db.Clauses(clause.OnConflict{DoNothing: true}).Create(&created)
	if insert.Error != nil {
		return insert.Error
	}
	if insert.RowsAffected != 1 {
		return nil
	}

	var blob model.WorkspaceBlob
	err = tx.workspaceLock(tx.db).Where("content_hash = ?", string(canonical)).Take(&blob).Error
	if err != nil {
		return workspaceNotFound(err)
	}
	if blob.RefCount < 0 {
		return &domain.WorkspaceBlobRefUnderflowError{
			ContentHash: canonical,
			RefCount:    blob.RefCount,
			RemoveCount: 0,
		}
	}
	if blob.RefCount == math.MaxInt64 {
		return &domain.WorkspaceCounterOverflowError{Counter: "ref_count"}
	}
	result := tx.db.Model(&model.WorkspaceBlob{}).Where("content_hash = ?", string(canonical)).
		Updates(map[string]any{
			"ref_count":       blob.RefCount + 1,
			"unreferenced_at": nil,
			"updated_at":      now,
		})
	if result.Error != nil {
		return result.Error
	}
	if result.RowsAffected == 0 {
		return domain.ErrWorkspaceRecordNotFound
	}
	return nil
}

func (tx *workspaceTx) RemoveBlobRefs(ownerType, ownerKey string, now time.Time) error {
	if err := workspaceValidateBlobRefOwner(ownerType, ownerKey); err != nil {
		return err
	}
	var refs []model.WorkspaceBlobRef
	if err := tx.workspaceLock(tx.db).
		Where("owner_type = ? AND owner_key = ?", ownerType, ownerKey).
		Find(&refs).Error; err != nil {
		return err
	}
	if len(refs) == 0 {
		return nil
	}

	removeCounts := make(map[string]int64)
	for i := range refs {
		removeCounts[refs[i].ContentHash]++
	}
	hashes := workspaceSortedContentHashes(removeCounts)
	blobs := make(map[string]model.WorkspaceBlob, len(removeCounts))
	for _, hash := range hashes {
		removeCount := removeCounts[hash]
		var blob model.WorkspaceBlob
		err := tx.workspaceLock(tx.db).Where("content_hash = ?", hash).Take(&blob).Error
		if err != nil {
			return workspaceNotFound(err)
		}
		if blob.RefCount < removeCount {
			return &domain.WorkspaceBlobRefUnderflowError{
				ContentHash: dto.WorkspaceContentHash(hash),
				RefCount:    blob.RefCount,
				RemoveCount: removeCount,
			}
		}
		blobs[hash] = blob
	}

	if err := tx.db.Where("owner_type = ? AND owner_key = ?", ownerType, ownerKey).
		Delete(&model.WorkspaceBlobRef{}).Error; err != nil {
		return err
	}
	for _, hash := range hashes {
		removeCount := removeCounts[hash]
		refCount := blobs[hash].RefCount - removeCount
		var unreferencedAt any
		if refCount == 0 {
			unreferencedAt = now
		}
		result := tx.db.Model(&model.WorkspaceBlob{}).Where("content_hash = ?", hash).
			Updates(map[string]any{
				"ref_count":       refCount,
				"unreferenced_at": unreferencedAt,
				"updated_at":      now,
			})
		if result.Error != nil {
			return result.Error
		}
		if result.RowsAffected == 0 {
			return domain.ErrWorkspaceRecordNotFound
		}
	}
	return nil
}

func (tx *workspaceTx) DeleteBlob(hash dto.WorkspaceContentHash) error {
	canonical, err := workspaceValidateHash(hash)
	if err != nil {
		return err
	}
	var blob model.WorkspaceBlob
	err = tx.workspaceLock(tx.db).Where("content_hash = ?", string(canonical)).Take(&blob).Error
	if err != nil {
		return workspaceNotFound(err)
	}
	if blob.RefCount < 0 {
		return &domain.WorkspaceCounterUnderflowError{Counter: "ref_count", Value: blob.RefCount}
	}
	if blob.RefCount != 0 {
		return &domain.WorkspaceBlobInUseError{ContentHash: canonical, RefCount: blob.RefCount}
	}
	result := tx.db.Where("content_hash = ?", string(canonical)).Delete(&model.WorkspaceBlob{})
	if result.Error != nil {
		return result.Error
	}
	if result.RowsAffected == 0 {
		return domain.ErrWorkspaceRecordNotFound
	}
	return nil
}

func (tx *workspaceTx) ClaimBlobForGC(
	hash dto.WorkspaceContentHash,
	unreferencedBefore time.Time,
) (bool, error) {
	canonical, err := workspaceValidateHash(hash)
	if err != nil {
		return false, err
	}
	var blob model.WorkspaceBlob
	err = tx.workspaceLock(tx.db).Where("content_hash = ?", string(canonical)).Take(&blob).Error
	if err != nil {
		return false, workspaceNotFound(err)
	}
	if blob.RefCount < 0 {
		return false, &domain.WorkspaceCounterUnderflowError{Counter: "ref_count", Value: blob.RefCount}
	}
	if blob.RefCount != 0 || blob.UnreferencedAt == nil || !blob.UnreferencedAt.Before(unreferencedBefore) {
		return false, nil
	}
	result := tx.db.Where(
		"content_hash = ? AND ref_count = 0 AND unreferenced_at < ?",
		string(canonical),
		unreferencedBefore,
	).Delete(&model.WorkspaceBlob{})
	if result.Error != nil {
		return false, result.Error
	}
	return result.RowsAffected == 1, nil
}

func (tx *workspaceTx) ReconcileBlobRefCounts(now time.Time) error {
	return tx.db.Exec(`
		UPDATE workspace_blob
		SET ref_count = (
				SELECT COUNT(*)
				FROM workspace_blob_ref
				WHERE workspace_blob_ref.content_hash = workspace_blob.content_hash
			),
			unreferenced_at = CASE
				WHEN EXISTS (
					SELECT 1
					FROM workspace_blob_ref
					WHERE workspace_blob_ref.content_hash = workspace_blob.content_hash
				) THEN NULL
				ELSE COALESCE(workspace_blob.unreferenced_at, ?)
			END,
			updated_at = ?
		WHERE workspace_blob.ref_count <> (
				SELECT COUNT(*)
				FROM workspace_blob_ref
				WHERE workspace_blob_ref.content_hash = workspace_blob.content_hash
			)
			OR (
				workspace_blob.unreferenced_at IS NOT NULL
				AND EXISTS (
					SELECT 1
					FROM workspace_blob_ref
					WHERE workspace_blob_ref.content_hash = workspace_blob.content_hash
				)
			)
			OR (
				workspace_blob.unreferenced_at IS NULL
				AND NOT EXISTS (
					SELECT 1
					FROM workspace_blob_ref
					WHERE workspace_blob_ref.content_hash = workspace_blob.content_hash
				)
			)
	`, now, now).Error
}

func workspaceModel(record domain.WorkspaceRecord) model.Workspace {
	return model.Workspace{
		ID:                  record.ID,
		WorkspaceID:         record.WorkspaceID,
		GlobalRevision:      uint64(record.GlobalRevision),
		ReplayFloorRevision: uint64(record.ReplayFloorRevision),
		LivePathCount:       record.LivePathCount,
		LiveBytes:           record.LiveBytes,
		CreatedAt:           record.CreatedAt,
		UpdatedAt:           record.UpdatedAt,
	}
}

func workspacePathModel(
	record domain.WorkspacePathRecord,
	canonical dto.WorkspacePath,
	pathKey string,
) model.WorkspacePath {
	return model.WorkspacePath{
		ID:           record.ID,
		WorkspaceID:  record.WorkspaceID,
		PathKey:      pathKey,
		Path:         string(canonical),
		PathRevision: uint64(record.PathRevision),
		Kind:         string(record.Kind),
		ContentHash:  workspaceHashToModel(record.ContentHash),
		Size:         record.Size,
		ModifiedAtMS: record.ModifiedAtMS,
		Executable:   record.Executable,
		Tombstone:    record.Tombstone,
	}
}

func workspaceOperationModel(record domain.WorkspaceOperationRecord) model.WorkspaceOperation {
	return model.WorkspaceOperation{
		WorkspaceID:      record.WorkspaceID,
		ClientID:         record.ClientID,
		OperationID:      record.OperationID,
		RequestKind:      record.RequestKind,
		RequestDigest:    record.RequestDigest,
		State:            record.State,
		ResultAction:     workspaceString(record.ResultAction),
		ResultJSON:       workspaceBytes(record.ResultJSON),
		ConflictJSON:     workspaceBytes(record.ConflictJSON),
		RequiredHash:     workspaceHashToModel(record.RequiredHash),
		ConflictRevision: workspaceConflictRevisionToModel(record.ConflictRevision),
		ExpiresAt:        workspaceTime(record.ExpiresAt),
		CreatedAt:        record.CreatedAt,
		UpdatedAt:        record.UpdatedAt,
	}
}

func workspaceConflictModel(
	record domain.WorkspaceConflictRecord,
	canonical dto.WorkspacePath,
) model.WorkspaceConflict {
	return model.WorkspaceConflict{
		WorkspaceID:             record.WorkspaceID,
		ConflictID:              record.ConflictID,
		ConflictRevision:        workspaceConflictRevisionString(record.ConflictRevision),
		PathKey:                 workspacePathKey(canonical),
		Path:                    string(canonical),
		Kind:                    string(record.Kind),
		Status:                  record.Status,
		AncestorJSON:            workspaceBytes(record.AncestorJSON),
		CurrentJSON:             workspaceBytes(record.CurrentJSON),
		IncomingJSON:            workspaceBytes(record.IncomingJSON),
		RenameTargetJSON:        workspaceBytes(record.RenameTargetJSON),
		CreatedByOperationID:    record.CreatedByOperationID,
		ResolutionOperationID:   workspaceString(record.ResolutionOperationID),
		ResolutionRevision:      workspaceRevisionToModel(record.ResolutionRevision),
		ResolutionChoice:        workspaceChoiceToModel(record.ResolutionChoice),
		ResolutionPathStateJSON: workspaceBytes(record.ResolutionPathStateJSON),
		ResolvedByClientID:      workspaceString(record.ResolvedByClientID),
		ResolvedAt:              workspaceTime(record.ResolvedAt),
		CreatedAt:               record.CreatedAt,
		UpdatedAt:               record.UpdatedAt,
	}
}

func workspaceBlobModel(record domain.WorkspaceBlobRecord) model.WorkspaceBlob {
	return model.WorkspaceBlob{
		ContentHash:    string(record.ContentHash),
		Size:           record.Size,
		UTF8Valid:      record.UTF8Valid,
		RefCount:       record.RefCount,
		UnreferencedAt: workspaceTime(record.UnreferencedAt),
		CreatedAt:      record.CreatedAt,
		UpdatedAt:      record.UpdatedAt,
	}
}

func workspaceValidateBlobRefOwner(ownerType, ownerKey string) error {
	switch ownerType {
	case "path", "event", "conflict":
	default:
		return fmt.Errorf("invalid workspace blob reference owner type %q", ownerType)
	}
	if ownerKey == "" || len(ownerKey) > 128 {
		return fmt.Errorf("invalid workspace blob reference owner key length %d", len(ownerKey))
	}
	return nil
}

func workspaceSortedContentHashes(counts map[string]int64) []string {
	hashes := make([]string, 0, len(counts))
	for hash := range counts {
		hashes = append(hashes, hash)
	}
	sort.Strings(hashes)
	return hashes
}

var _ domain.WorkspaceRepository = (*workspaceRepository)(nil)
var _ domain.WorkspaceWriteTx = (*workspaceTx)(nil)
