package domain

import (
	"context"
	"errors"
	"fmt"
	"time"

	"github.com/haierkeys/fast-note-sync-service/internal/dto"
)

var (
	ErrWorkspaceRecordNotFound = errors.New("workspace record not found")
	ErrWorkspaceInvalidUID     = errors.New("workspace uid must be positive")
)

type WorkspaceUniqueConstraintError struct {
	Entity string
	Key    string
}

type WorkspaceOperationImmutableError struct {
	ClientID    string
	OperationID string
	State       string
}

type WorkspaceConflictImmutableError struct {
	ConflictID string
	Status     string
}

func (e *WorkspaceConflictImmutableError) Error() string {
	return fmt.Sprintf("workspace conflict is immutable (%s, status=%s)", e.ConflictID, e.Status)
}

func (e *WorkspaceOperationImmutableError) Error() string {
	return fmt.Sprintf(
		"workspace operation is immutable (%s/%s, state=%s)",
		e.ClientID,
		e.OperationID,
		e.State,
	)
}

func (e *WorkspaceUniqueConstraintError) Error() string {
	return fmt.Sprintf("workspace %s already exists (%s)", e.Entity, e.Key)
}

type WorkspacePathKeyCollisionError struct {
	WorkspaceID   string
	PathKey       string
	RequestedPath string
	StoredPath    string
}

func (e *WorkspacePathKeyCollisionError) Error() string {
	return fmt.Sprintf("workspace path key collision in %s for %s", e.WorkspaceID, e.PathKey)
}

type WorkspaceBlobRefUnderflowError struct {
	ContentHash dto.WorkspaceContentHash
	RefCount    int64
	RemoveCount int64
}

type WorkspaceCounterUnderflowError struct {
	Counter string
	Value   int64
}

func (e *WorkspaceCounterUnderflowError) Error() string {
	return fmt.Sprintf("workspace counter %s cannot be negative: %d", e.Counter, e.Value)
}

type WorkspaceCounterOverflowError struct {
	Counter string
}

func (e *WorkspaceCounterOverflowError) Error() string {
	return "workspace counter overflow: " + e.Counter
}

type WorkspaceBlobInUseError struct {
	ContentHash dto.WorkspaceContentHash
	RefCount    int64
}

func (e *WorkspaceBlobInUseError) Error() string {
	return fmt.Sprintf("workspace blob %s is still referenced (%d)", e.ContentHash, e.RefCount)
}

func (e *WorkspaceBlobRefUnderflowError) Error() string {
	return fmt.Sprintf(
		"workspace blob reference count underflow for %s: have %d, removing %d",
		e.ContentHash,
		e.RefCount,
		e.RemoveCount,
	)
}

type WorkspaceRecord struct {
	ID                  int64
	WorkspaceID         string
	GlobalRevision      dto.WorkspaceRevision
	ReplayFloorRevision dto.WorkspaceRevision
	LivePathCount       int64
	LiveBytes           uint64
	CreatedAt           time.Time
	UpdatedAt           time.Time
}

type WorkspaceClientRecord struct {
	WorkspaceID     string
	ClientID        string
	LastAckRevision dto.WorkspaceRevision
	LastSeenAt      time.Time
}

type WorkspacePathRecord struct {
	ID           int64
	WorkspaceID  string
	Path         dto.WorkspacePath
	PathRevision dto.WorkspaceRevision
	Kind         dto.WorkspaceEntryKind
	ContentHash  *dto.WorkspaceContentHash
	Size         uint64
	ModifiedAtMS int64
	Executable   bool
	Tombstone    bool
}

type WorkspaceEventRecord struct {
	ID               int64
	WorkspaceID      string
	Revision         dto.WorkspaceRevision
	Kind             string
	OperationID      string
	OriginClientID   string
	MutationJSON     []byte
	PathStateJSON    []byte
	OldPathStateJSON []byte
	NewPathStateJSON []byte
	ResolvedJSON     []byte
	CreatedAt        time.Time
}

type WorkspaceOperationRecord struct {
	WorkspaceID      string
	ClientID         string
	OperationID      string
	RequestKind      string
	RequestDigest    string
	State            string
	ResultAction     *string
	ResultJSON       []byte
	ConflictJSON     []byte
	RequiredHash     *dto.WorkspaceContentHash
	ConflictRevision *dto.WorkspaceConflictRevision
	ExpiresAt        *time.Time
	CreatedAt        time.Time
	UpdatedAt        time.Time
}

type WorkspaceConflictRecord struct {
	WorkspaceID             string
	ConflictID              string
	ConflictRevision        dto.WorkspaceConflictRevision
	Path                    dto.WorkspacePath
	Kind                    dto.WorkspaceConflictKind
	Status                  string
	AncestorJSON            []byte
	CurrentJSON             []byte
	IncomingJSON            []byte
	RenameTargetJSON        []byte
	CreatedByOperationID    string
	ResolutionOperationID   *string
	ResolutionRevision      *dto.WorkspaceRevision
	ResolutionChoice        *dto.WorkspaceConflictChoice
	ResolutionPathStateJSON []byte
	ResolvedByClientID      *string
	ResolvedAt              *time.Time
	CreatedAt               time.Time
	UpdatedAt               time.Time
}

type WorkspaceBlobRecord struct {
	ContentHash    dto.WorkspaceContentHash
	Size           uint64
	UTF8Valid      bool
	RefCount       int64
	UnreferencedAt *time.Time
	CreatedAt      time.Time
	UpdatedAt      time.Time
}

type WorkspaceBlobRefRecord struct {
	ID          int64
	ContentHash dto.WorkspaceContentHash
	OwnerType   string
	OwnerKey    string
	CreatedAt   time.Time
	UpdatedAt   time.Time
}

type WorkspaceStoredEvent struct {
	Revision       dto.WorkspaceRevision
	OperationID    dto.WorkspaceUUID
	OriginClientID dto.WorkspaceUUID
	Mutation       dto.WorkspaceMutation
	PathState      dto.WorkspacePathState
	OldPathState   *dto.WorkspacePathState
	NewPathState   *dto.WorkspacePathState
}

type WorkspaceRevisionItem struct {
	Revision         dto.WorkspaceRevision
	Event            *WorkspaceStoredEvent
	ConflictResolved *dto.WorkspaceConflictResolvedMessage
}

type WorkspacePendingConflictCursor interface {
	Next(ctx context.Context) (*dto.WorkspaceConflictCreatedMessage, error)
	Close() error
}

type WorkspaceChangeSet struct {
	Mode             dto.WorkspaceSnapshotMode
	FromRevision     dto.WorkspaceRevision
	FinalRevision    dto.WorkspaceRevision
	Entries          []dto.WorkspacePathState
	Events           []WorkspaceStoredEvent
	RevisionItems    []WorkspaceRevisionItem
	PendingConflicts WorkspacePendingConflictCursor
	EntryCount       uint32
	EventCount       uint32
	ConflictCount    uint32
}

type WorkspaceRepository interface {
	Migrate(ctx context.Context, uid int64) error
	Read(ctx context.Context, uid int64, fn func(WorkspaceReadTx) error) error
	OpenReadSnapshot(ctx context.Context, uid int64) (WorkspaceReadSnapshot, error)
	Write(ctx context.Context, uid int64, fn func(WorkspaceWriteTx) error) error
}

type WorkspaceReadSnapshot interface {
	WorkspaceReadTx
	Close() error
}

type WorkspaceReadTx interface {
	Workspaces() ([]WorkspaceRecord, error)
	WorkspacesPage(afterID int64, limit int) ([]WorkspaceRecord, error)
	Workspace(workspaceID string) (*WorkspaceRecord, error)
	Client(workspaceID, clientID string) (*WorkspaceClientRecord, error)
	Path(workspaceID string, path dto.WorkspacePath) (*WorkspacePathRecord, error)
	Paths(workspaceID string) ([]WorkspacePathRecord, error)
	EventsAfter(workspaceID string, after, through dto.WorkspaceRevision) ([]WorkspaceEventRecord, error)
	Operation(clientID, operationID string) (*WorkspaceOperationRecord, error)
	Conflict(workspaceID, conflictID string) (*WorkspaceConflictRecord, error)
	PendingConflict(workspaceID string, path dto.WorkspacePath) (*WorkspaceConflictRecord, error)
	PendingConflicts(workspaceID string) ([]WorkspaceConflictRecord, error)
	PendingConflictCount(workspaceID string) (int64, error)
	PendingConflictBoundary(workspaceID string) (string, error)
	PendingConflictPage(workspaceID, afterID, throughID string, limit int) ([]WorkspaceConflictRecord, error)
	Operations(workspaceID string) ([]WorkspaceOperationRecord, error)
	ExpiredWaitingOperations(workspaceID string, before time.Time, limit int) ([]WorkspaceOperationRecord, error)
	Clients(workspaceID string) ([]WorkspaceClientRecord, error)
	ClientsPage(workspaceID, afterClientID string, limit int) ([]WorkspaceClientRecord, error)
	Tombstones(workspaceID string, through dto.WorkspaceRevision, limit int) ([]WorkspacePathRecord, error)
	PrunableEvents(workspaceID string, olderThan time.Time, atOrBefore dto.WorkspaceRevision, limit int) ([]WorkspaceEventRecord, error)
	Blob(hash dto.WorkspaceContentHash) (*WorkspaceBlobRecord, error)
	BlobsAfter(after *dto.WorkspaceContentHash, limit int) ([]WorkspaceBlobRecord, error)
}

type WorkspaceWriteTx interface {
	WorkspaceReadTx
	CreateWorkspace(record WorkspaceRecord) error
	SaveWorkspace(record WorkspaceRecord) error
	SaveClient(record WorkspaceClientRecord) error
	SavePath(record WorkspacePathRecord) error
	DeletePath(recordID int64) error
	SaveEvent(record WorkspaceEventRecord) error
	DeleteEvents(ids []int64) error
	SaveOperation(record WorkspaceOperationRecord) error
	SaveConflict(record WorkspaceConflictRecord) error
	SaveBlob(record WorkspaceBlobRecord) error
	AddBlobRef(ref WorkspaceBlobRefRecord, now time.Time) error
	RemoveBlobRefs(ownerType, ownerKey string, now time.Time) error
	DeleteBlob(hash dto.WorkspaceContentHash) error
	ClaimBlobForGC(hash dto.WorkspaceContentHash, unreferencedBefore time.Time) (bool, error)
	ReconcileBlobRefCounts(now time.Time) error
}
