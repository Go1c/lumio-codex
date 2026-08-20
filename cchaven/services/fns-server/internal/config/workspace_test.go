package config

import (
	"path/filepath"
	"testing"

	"github.com/creasty/defaults"
	"github.com/stretchr/testify/require"
)

func TestWorkspaceConfigDefaultsAndValidation(t *testing.T) {
	var cfg WorkspaceConfig
	require.NoError(t, defaults.Set(&cfg))
	require.Equal(t, "storage/workspace-blobs", cfg.BlobPath)
	require.Equal(t, 50_000, cfg.MaxPaths)
	require.Equal(t, uint64(5_368_709_120), cfg.MaxBytes)
	require.Equal(t, 100_000, cfg.EventMaxPerWorkspace)
	require.Equal(t, "30d", cfg.EventRetention)
	require.Equal(t, "24h", cfg.BlobGCGrace)
	require.Equal(t, "24h", cfg.StagingTTL)
	require.Equal(t, 500, cfg.PruneBatchSize)
	require.NoError(t, cfg.Validate())
}

func TestWorkspaceConfigRejectsUnsafeOrZeroValues(t *testing.T) {
	valid := defaultWorkspaceConfigForTest(t)
	cases := map[string]func(*WorkspaceConfig){
		"empty blob path":        func(c *WorkspaceConfig) { c.BlobPath = "" },
		"nul in blob path":       func(c *WorkspaceConfig) { c.BlobPath = "workspace\x00blobs" },
		"zero max paths":         func(c *WorkspaceConfig) { c.MaxPaths = 0 },
		"too many paths":         func(c *WorkspaceConfig) { c.MaxPaths = 50_001 },
		"zero max bytes":         func(c *WorkspaceConfig) { c.MaxBytes = 0 },
		"too many bytes":         func(c *WorkspaceConfig) { c.MaxBytes = 5_368_709_121 },
		"zero event maximum":     func(c *WorkspaceConfig) { c.EventMaxPerWorkspace = 0 },
		"invalid retention":      func(c *WorkspaceConfig) { c.EventRetention = "thirty days" },
		"zero retention":         func(c *WorkspaceConfig) { c.EventRetention = "0s" },
		"zero gc grace":          func(c *WorkspaceConfig) { c.BlobGCGrace = "0s" },
		"zero staging ttl":       func(c *WorkspaceConfig) { c.StagingTTL = "0s" },
		"zero batch":             func(c *WorkspaceConfig) { c.PruneBatchSize = 0 },
		"batch above safe limit": func(c *WorkspaceConfig) { c.PruneBatchSize = 10_001 },
	}
	for name, mutate := range cases {
		t.Run(name, func(t *testing.T) {
			cfg := valid
			mutate(&cfg)
			require.Error(t, cfg.Validate())
		})
	}
}

func TestWorkspaceConfigTransportDefaults(t *testing.T) {
	var cfg WorkspaceConfig
	require.NoError(t, defaults.Set(&cfg))
	require.Equal(t, 8, cfg.MaxWorkspacesPerUser)
	require.Empty(t, cfg.Roots)
}

func TestWorkspaceConfigRejectsInvalidRootAuthorization(t *testing.T) {
	root, err := filepath.EvalSymlinks(t.TempDir())
	require.NoError(t, err)
	valid := defaultWorkspaceConfigForTest(t)
	valid.Roots = []WorkspaceRootConfig{{
		UID:         41,
		WorkspaceID: "10000000-0000-4000-8000-000000000001",
		Root:        root,
	}}
	require.NoError(t, valid.Validate())

	cases := map[string]func(*WorkspaceConfig){
		"non-positive uid":       func(c *WorkspaceConfig) { c.Roots[0].UID = 0 },
		"invalid workspace uuid": func(c *WorkspaceConfig) { c.Roots[0].WorkspaceID = "not-a-uuid" },
		"relative root":          func(c *WorkspaceConfig) { c.Roots[0].Root = "projects/demo" },
		"filesystem root":        func(c *WorkspaceConfig) { c.Roots[0].Root = string(filepath.Separator) },
		"duplicate mapping":      func(c *WorkspaceConfig) { c.Roots = append(c.Roots, c.Roots[0]) },
		"zero workspace cap":     func(c *WorkspaceConfig) { c.MaxWorkspacesPerUser = 0 },
		"over workspace cap": func(c *WorkspaceConfig) {
			c.MaxWorkspacesPerUser = 1
			c.Roots = append(c.Roots, WorkspaceRootConfig{
				UID:         41,
				WorkspaceID: "20000000-0000-4000-8000-000000000002",
				Root:        filepath.Join(root, "child"),
			})
		},
	}
	for name, mutate := range cases {
		t.Run(name, func(t *testing.T) {
			candidate := valid
			candidate.Roots = append([]WorkspaceRootConfig(nil), valid.Roots...)
			mutate(&candidate)
			require.Error(t, candidate.Validate())
		})
	}
}

func defaultWorkspaceConfigForTest(t *testing.T) WorkspaceConfig {
	t.Helper()
	var cfg WorkspaceConfig
	require.NoError(t, defaults.Set(&cfg))
	return cfg
}
