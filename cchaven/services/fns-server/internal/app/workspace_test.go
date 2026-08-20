package app

import (
	"context"
	"errors"
	"testing"
	"time"

	"github.com/stretchr/testify/require"
	"go.uber.org/zap"
	"go.uber.org/zap/zaptest/observer"
)

func TestSyncTokenChangeNotifiesWorkspaceV2Lifecycle(t *testing.T) {
	lifecycle := &workspaceV2TokenLifecycleProbe{}
	a := &App{workspaceV2: lifecycle}

	a.syncTokenChange(41, 7, "p:ws", true)
	require.Equal(t, []int64{41}, lifecycle.kickedUIDs)
	require.Equal(t, []int64{7}, lifecycle.kickedTokenIDs)

	a.syncTokenChange(41, 7, "p:ws c:fns-agent", false)
	require.Equal(t, []string{"p:ws c:fns-agent"}, lifecycle.scopes)
}

type workspaceV2TokenLifecycleProbe struct {
	kickedUIDs     []int64
	kickedTokenIDs []int64
	scopes         []string
	closeCalls     int
	waitCalls      int
	waitErr        error
}

func (p *workspaceV2TokenLifecycleProbe) Close() { p.closeCalls++ }

func (p *workspaceV2TokenLifecycleProbe) WaitAllClosed(_ time.Duration) error {
	p.waitCalls++
	return p.waitErr
}

func (p *workspaceV2TokenLifecycleProbe) KickToken(uid, tokenID int64) {
	p.kickedUIDs = append(p.kickedUIDs, uid)
	p.kickedTokenIDs = append(p.kickedTokenIDs, tokenID)
}

func (p *workspaceV2TokenLifecycleProbe) UpdateTokenScope(_ int64, _ int64, scope string) {
	p.scopes = append(p.scopes, scope)
}

func TestShutdownPropagatesWorkspaceV2WaitError(t *testing.T) {
	waitErr := errors.New("workspace lifecycle timed out")
	lifecycle := &workspaceV2TokenLifecycleProbe{waitErr: waitErr}
	core, logs := observer.New(zap.InfoLevel)
	a := &App{
		Infra:        &Infra{logger: zap.New(core)},
		Repositories: &Repositories{},
		Services:     &Services{},
		workspaceV2:  lifecycle,
		shutdownCh:   make(chan struct{}),
	}

	err := a.Shutdown(context.Background())
	require.ErrorContains(t, err, waitErr.Error())
	require.ErrorIs(t, err, waitErr)
	require.Equal(t, 1, lifecycle.closeCalls)
	require.Equal(t, 1, lifecycle.waitCalls)
	require.Equal(t, 0, logs.FilterMessage("All workspace v2 connections closed").Len(),
		"shutdown must not log false success after a workspace lifecycle timeout")
}

func TestInitServicesReturnsWorkspaceConfigurationErrorBeforeWiring(t *testing.T) {
	cfg := &AppConfig{}
	cfg.Workspace.BlobPath = ""

	services, err := initServices(cfg, nil, nil, nil)

	require.Nil(t, services)
	require.ErrorContains(t, err, "workspace blob-path must not be blank")
}
