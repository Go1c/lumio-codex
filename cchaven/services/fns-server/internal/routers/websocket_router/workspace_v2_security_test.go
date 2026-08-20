package websocket_router

import (
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/haierkeys/fast-note-sync-service/internal/config"
	"github.com/haierkeys/fast-note-sync-service/internal/dto"
	"github.com/stretchr/testify/require"
)

const (
	workspaceV2SecurityWorkspaceID = "10000000-0000-4000-8000-000000000001"
	workspaceV2SecurityOtherID     = "20000000-0000-4000-8000-000000000002"
)

func TestWorkspaceV2AccessPolicyAuthorizesOnlyMappedUIDAndWorkspace(t *testing.T) {
	root := canonicalTempDir(t)
	policy := NewWorkspaceV2AccessPolicy(workspaceV2SecurityConfig(root))
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)

	require.NoError(t, policy.Authorize(41, workspaceID))
	assertWorkspaceV2SecurityError(t, policy.Authorize(42, workspaceID), "forbidden", "workspaceId", "not_allowed")
	assertWorkspaceV2SecurityError(t, policy.Authorize(41, dto.WorkspaceUUID(workspaceV2SecurityOtherID)), "forbidden", "workspaceId", "not_allowed")
}

func TestWorkspaceV2AccessPolicyRejectsAbsoluteTraversalAndBackslashPaths(t *testing.T) {
	root := canonicalTempDir(t)
	policy := NewWorkspaceV2AccessPolicy(workspaceV2SecurityConfig(root))
	workspaceID := dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID)

	for _, path := range []dto.WorkspacePath{"../secret", "/absolute", "nested\\file"} {
		err := policy.CheckPath(41, workspaceID, path)
		assertWorkspaceV2SecurityError(t, err, "invalid_path", "data.path", "")
	}
}

func TestWorkspaceV2AccessPolicyRejectsSymlinkedRootOrAncestor(t *testing.T) {
	parent := canonicalTempDir(t)
	canonicalRoot := filepath.Join(parent, "root")
	require.NoError(t, os.Mkdir(canonicalRoot, 0o755))
	rootLink := filepath.Join(parent, "root-link")
	require.NoError(t, os.Symlink(canonicalRoot, rootLink))
	rootPolicy := NewWorkspaceV2AccessPolicy(workspaceV2SecurityConfig(rootLink))
	assertWorkspaceV2SecurityError(t, rootPolicy.CheckPath(41, dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), "file.md"), "invalid_path", "data.path", "root_symlink")

	inside := filepath.Join(canonicalRoot, "inside")
	require.NoError(t, os.Mkdir(inside, 0o755))
	childLink := filepath.Join(canonicalRoot, "child-link")
	require.NoError(t, os.Symlink(inside, childLink))
	childPolicy := NewWorkspaceV2AccessPolicy(workspaceV2SecurityConfig(childLink))
	assertWorkspaceV2SecurityError(t, childPolicy.CheckPath(41, dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), "missing.md"), "invalid_path", "data.path", "root_symlink")
}

func TestWorkspaceV2AccessPolicyRejectsExistingSymlinkOutsideRoot(t *testing.T) {
	parent := canonicalTempDir(t)
	root := filepath.Join(parent, "root")
	outside := filepath.Join(parent, "outside")
	require.NoError(t, os.Mkdir(root, 0o755))
	require.NoError(t, os.Mkdir(outside, 0o755))
	require.NoError(t, os.WriteFile(filepath.Join(outside, "secret.txt"), []byte("secret"), 0o600))
	require.NoError(t, os.Symlink(outside, filepath.Join(root, "link")))
	policy := NewWorkspaceV2AccessPolicy(workspaceV2SecurityConfig(root))

	err := policy.CheckPath(41, dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), "link/secret.txt")
	assertWorkspaceV2SecurityError(t, err, "invalid_path", "data.path", "outside_root")
	require.NotContains(t, err.Error(), root)
	require.NotContains(t, err.Error(), outside)
}

func TestWorkspaceV2AccessPolicyAllowsExistingSymlinkInsideRootAndMissingLeaf(t *testing.T) {
	root := canonicalTempDir(t)
	subdir := filepath.Join(root, "subdir")
	require.NoError(t, os.Mkdir(subdir, 0o755))
	require.NoError(t, os.Symlink(subdir, filepath.Join(root, "link")))
	policy := NewWorkspaceV2AccessPolicy(workspaceV2SecurityConfig(root))

	require.NoError(t, policy.CheckPath(41, dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), "link/missing/leaf.txt"))
}

func TestWorkspaceV2ConnectionChecksPathBeforeMutationOrResolution(t *testing.T) {
	parent := canonicalTempDir(t)
	root := filepath.Join(parent, "root")
	outside := filepath.Join(parent, "outside")
	require.NoError(t, os.Mkdir(root, 0o755))
	require.NoError(t, os.Mkdir(outside, 0o755))
	require.NoError(t, os.Symlink(outside, filepath.Join(root, "link")))

	connection := &workspaceV2Connection{
		server: &WorkspaceV2Server{access: NewWorkspaceV2AccessPolicy(workspaceV2SecurityConfig(root))},
		uid:    41,
	}
	err := connection.authorizeWorkspacePath(dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), "link/secret.txt")
	assertWorkspaceV2SecurityError(t, err, "invalid_path", "data.path", "outside_root")
}

func TestWorkspaceV2AccessPolicyNeverIncludesRootInReturnedError(t *testing.T) {
	root := canonicalTempDir(t)
	policy := NewWorkspaceV2AccessPolicy(workspaceV2SecurityConfig(root))
	err := policy.CheckPath(41, dto.WorkspaceUUID(workspaceV2SecurityWorkspaceID), "../outside")
	require.Error(t, err)
	require.NotContains(t, err.Error(), root)
}

func workspaceV2SecurityConfig(root string) config.WorkspaceConfig {
	return config.WorkspaceConfig{
		MaxWorkspacesPerUser: config.WorkspaceMaxPerUser,
		Roots: []config.WorkspaceRootConfig{{
			UID:         41,
			WorkspaceID: workspaceV2SecurityWorkspaceID,
			Root:        root,
		}},
	}
}

func canonicalTempDir(t *testing.T) string {
	t.Helper()
	path, err := filepath.EvalSymlinks(t.TempDir())
	require.NoError(t, err)
	return path
}

func assertWorkspaceV2SecurityError(t *testing.T, err error, code, field, reason string) {
	t.Helper()
	require.Error(t, err)
	var accessErr *WorkspaceV2AccessError
	require.ErrorAs(t, err, &accessErr)
	require.Equal(t, code, accessErr.Code)
	require.Equal(t, field, accessErr.Field)
	if reason != "" {
		require.Equal(t, reason, accessErr.Reason)
	}
	require.False(t, strings.Contains(accessErr.Error(), string(filepath.Separator)+"tmp"))
}
