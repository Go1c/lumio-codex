package model

import "time"

const (
	TableNameWorkspace          = "workspace"
	TableNameWorkspaceClient    = "workspace_client"
	TableNameWorkspacePath      = "workspace_path"
	TableNameWorkspaceEvent     = "workspace_event"
	TableNameWorkspaceOperation = "workspace_operation"
	TableNameWorkspaceConflict  = "workspace_conflict"
	TableNameWorkspaceBlob      = "workspace_blob"
	TableNameWorkspaceBlobRef   = "workspace_blob_ref"
)

type Workspace struct {
	ID                  int64     `gorm:"column:id;primaryKey;autoIncrement;not null"`
	WorkspaceID         string    `gorm:"column:workspace_id;type:varchar(36);not null;uniqueIndex:idx_workspace_workspace_id"`
	GlobalRevision      uint64    `gorm:"column:global_revision;not null;default:0"`
	ReplayFloorRevision uint64    `gorm:"column:replay_floor_revision;not null;default:0"`
	LivePathCount       int64     `gorm:"column:live_path_count;not null;default:0"`
	LiveBytes           uint64    `gorm:"column:live_bytes;not null;default:0"`
	CreatedAt           time.Time `gorm:"column:created_at;not null;autoCreateTime"`
	UpdatedAt           time.Time `gorm:"column:updated_at;not null;autoUpdateTime"`
}

func (*Workspace) TableName() string { return TableNameWorkspace }

type WorkspaceClient struct {
	WorkspaceID     string    `gorm:"column:workspace_id;type:varchar(36);not null;uniqueIndex:idx_workspace_client_identity,priority:1"`
	ClientID        string    `gorm:"column:client_id;type:varchar(36);not null;uniqueIndex:idx_workspace_client_identity,priority:2"`
	LastAckRevision uint64    `gorm:"column:last_ack_revision;not null;default:0"`
	LastSeenAt      time.Time `gorm:"column:last_seen_at;not null"`
}

func (*WorkspaceClient) TableName() string { return TableNameWorkspaceClient }

type WorkspacePath struct {
	ID           int64   `gorm:"column:id;primaryKey;autoIncrement;not null"`
	WorkspaceID  string  `gorm:"column:workspace_id;type:varchar(36);not null;uniqueIndex:idx_workspace_path_identity,priority:1;index:idx_workspace_path_revision,priority:1"`
	PathKey      string  `gorm:"column:path_key;type:char(64);not null;uniqueIndex:idx_workspace_path_identity,priority:2"`
	Path         string  `gorm:"column:path;type:text;not null"`
	PathRevision uint64  `gorm:"column:path_revision;not null;default:0;index:idx_workspace_path_revision,priority:2"`
	Kind         string  `gorm:"column:kind;type:varchar(16);not null"`
	ContentHash  *string `gorm:"column:content_hash;type:varchar(71)"`
	Size         uint64  `gorm:"column:size;not null;default:0"`
	ModifiedAtMS int64   `gorm:"column:modified_at_ms;not null;default:0"`
	Executable   bool    `gorm:"column:executable;not null;default:false"`
	Tombstone    bool    `gorm:"column:tombstone;not null;default:false"`
}

func (*WorkspacePath) TableName() string { return TableNameWorkspacePath }

type WorkspaceEvent struct {
	ID               int64     `gorm:"column:id;primaryKey;autoIncrement;not null"`
	WorkspaceID      string    `gorm:"column:workspace_id;type:varchar(36);not null;uniqueIndex:idx_workspace_event_revision,priority:1;index:idx_workspace_event_created_at,priority:1"`
	Revision         uint64    `gorm:"column:revision;not null;uniqueIndex:idx_workspace_event_revision,priority:2"`
	Kind             string    `gorm:"column:kind;type:varchar(24);not null;default:event"`
	OperationID      string    `gorm:"column:operation_id;type:varchar(36);not null"`
	OriginClientID   string    `gorm:"column:origin_client_id;type:varchar(36);not null"`
	MutationJSON     []byte    `gorm:"column:mutation_json;type:text;not null"`
	PathStateJSON    []byte    `gorm:"column:path_state_json;type:text;not null"`
	OldPathStateJSON []byte    `gorm:"column:old_path_state_json;type:text"`
	NewPathStateJSON []byte    `gorm:"column:new_path_state_json;type:text"`
	ResolvedJSON     []byte    `gorm:"column:resolved_json;type:text"`
	CreatedAt        time.Time `gorm:"column:created_at;not null;autoCreateTime;index:idx_workspace_event_created_at,priority:2"`
}

func (*WorkspaceEvent) TableName() string { return TableNameWorkspaceEvent }

type WorkspaceOperation struct {
	WorkspaceID      string     `gorm:"column:workspace_id;type:varchar(36);not null"`
	ClientID         string     `gorm:"column:client_id;type:varchar(36);not null;uniqueIndex:idx_workspace_operation_identity,priority:1"`
	OperationID      string     `gorm:"column:operation_id;type:varchar(36);not null;uniqueIndex:idx_workspace_operation_identity,priority:2"`
	RequestKind      string     `gorm:"column:request_kind;type:varchar(32);not null"`
	RequestDigest    string     `gorm:"column:request_digest;type:char(64);not null"`
	State            string     `gorm:"column:state;type:varchar(16);not null;check:chk_workspace_operation_state,state IN ('waiting_blob','terminal','expired_guard')"`
	ResultAction     *string    `gorm:"column:result_action;type:varchar(64)"`
	ResultJSON       []byte     `gorm:"column:result_json;type:text"`
	ConflictJSON     []byte     `gorm:"column:conflict_json;type:text"`
	RequiredHash     *string    `gorm:"column:required_hash;type:varchar(71)"`
	ConflictRevision *string    `gorm:"column:conflict_revision;type:varchar(20)"`
	ExpiresAt        *time.Time `gorm:"column:expires_at"`
	CreatedAt        time.Time  `gorm:"column:created_at;not null;autoCreateTime"`
	UpdatedAt        time.Time  `gorm:"column:updated_at;not null;autoUpdateTime"`
}

func (*WorkspaceOperation) TableName() string { return TableNameWorkspaceOperation }

type WorkspaceConflict struct {
	WorkspaceID             string     `gorm:"column:workspace_id;type:varchar(36);not null;index:idx_workspace_conflict_status,priority:1"`
	ConflictID              string     `gorm:"column:conflict_id;type:varchar(36);not null;uniqueIndex:idx_workspace_conflict_id"`
	ConflictRevision        string     `gorm:"column:conflict_revision;type:varchar(20);not null"`
	PathKey                 string     `gorm:"column:path_key;type:char(64);not null"`
	Path                    string     `gorm:"column:path;type:text;not null"`
	Kind                    string     `gorm:"column:kind;type:varchar(24);not null"`
	Status                  string     `gorm:"column:status;type:varchar(16);not null;index:idx_workspace_conflict_status,priority:2"`
	AncestorJSON            []byte     `gorm:"column:ancestor_json;type:text;not null"`
	CurrentJSON             []byte     `gorm:"column:current_json;type:text;not null"`
	IncomingJSON            []byte     `gorm:"column:incoming_json;type:text;not null"`
	RenameTargetJSON        []byte     `gorm:"column:rename_target_json;type:text"`
	CreatedByOperationID    string     `gorm:"column:created_by_operation_id;type:varchar(36);not null"`
	ResolutionOperationID   *string    `gorm:"column:resolution_operation_id;type:varchar(36)"`
	ResolutionRevision      *uint64    `gorm:"column:resolution_revision"`
	ResolutionChoice        *string    `gorm:"column:resolution_choice;type:varchar(16)"`
	ResolutionPathStateJSON []byte     `gorm:"column:resolution_path_state_json;type:text"`
	ResolvedByClientID      *string    `gorm:"column:resolved_by_client_id;type:varchar(36)"`
	ResolvedAt              *time.Time `gorm:"column:resolved_at"`
	CreatedAt               time.Time  `gorm:"column:created_at;not null;autoCreateTime"`
	UpdatedAt               time.Time  `gorm:"column:updated_at;not null;autoUpdateTime;index:idx_workspace_conflict_status,priority:3"`
}

func (*WorkspaceConflict) TableName() string { return TableNameWorkspaceConflict }

type WorkspaceBlob struct {
	ContentHash    string     `gorm:"column:content_hash;type:varchar(71);primaryKey;not null"`
	Size           uint64     `gorm:"column:size;not null;default:0"`
	UTF8Valid      bool       `gorm:"column:utf8_valid;not null;default:false"`
	RefCount       int64      `gorm:"column:ref_count;not null;default:0"`
	UnreferencedAt *time.Time `gorm:"column:unreferenced_at"`
	CreatedAt      time.Time  `gorm:"column:created_at;not null;autoCreateTime"`
	UpdatedAt      time.Time  `gorm:"column:updated_at;not null;autoUpdateTime"`
}

func (*WorkspaceBlob) TableName() string { return TableNameWorkspaceBlob }

type WorkspaceBlobRef struct {
	ID          int64     `gorm:"column:id;primaryKey;autoIncrement;not null"`
	ContentHash string    `gorm:"column:content_hash;type:varchar(71);not null;uniqueIndex:idx_workspace_blob_ref_owner,priority:3;index:idx_workspace_blob_ref_content_hash"`
	OwnerType   string    `gorm:"column:owner_type;type:varchar(16);not null;uniqueIndex:idx_workspace_blob_ref_owner,priority:1;check:chk_workspace_blob_ref_owner_type,owner_type IN ('path','event','conflict')"`
	OwnerKey    string    `gorm:"column:owner_key;type:varchar(128);not null;uniqueIndex:idx_workspace_blob_ref_owner,priority:2"`
	CreatedAt   time.Time `gorm:"column:created_at;not null;autoCreateTime"`
	UpdatedAt   time.Time `gorm:"column:updated_at;not null;autoUpdateTime"`
}

func (*WorkspaceBlobRef) TableName() string { return TableNameWorkspaceBlobRef }
