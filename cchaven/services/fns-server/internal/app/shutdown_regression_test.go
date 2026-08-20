package app

import (
	"context"
	"errors"
	"sync"
	"testing"
	"time"

	"github.com/stretchr/testify/require"
	"go.uber.org/zap"
)

type shutdownRegressionError struct {
	message string
}

func (e *shutdownRegressionError) Error() string { return e.message }

type blockingWorkspaceV2Lifecycle struct {
	mu          sync.Mutex
	startOnce   sync.Once
	started     chan struct{}
	release     chan struct{}
	releaseOnce sync.Once
	waitErr     error
	closeCalls  int
	waitCalls   int
}

func newBlockingWorkspaceV2Lifecycle(waitErr error) *blockingWorkspaceV2Lifecycle {
	return &blockingWorkspaceV2Lifecycle{
		started: make(chan struct{}),
		release: make(chan struct{}),
		waitErr: waitErr,
	}
}

func (p *blockingWorkspaceV2Lifecycle) Close() {
	p.mu.Lock()
	p.closeCalls++
	p.mu.Unlock()
}

func (p *blockingWorkspaceV2Lifecycle) WaitAllClosed(time.Duration) error {
	p.mu.Lock()
	p.waitCalls++
	p.mu.Unlock()
	p.startOnce.Do(func() { close(p.started) })
	<-p.release
	return p.waitErr
}

func (p *blockingWorkspaceV2Lifecycle) releaseWait() {
	p.releaseOnce.Do(func() { close(p.release) })
}

func (p *blockingWorkspaceV2Lifecycle) calls() (int, int) {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.closeCalls, p.waitCalls
}

func TestShutdownConcurrentAndRepeatedCallersShareOneResult(t *testing.T) {
	tests := []struct {
		name    string
		waitErr error
	}{
		{name: "success"},
		{name: "error", waitErr: &shutdownRegressionError{message: "workspace shutdown timed out"}},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			lifecycle := newBlockingWorkspaceV2Lifecycle(test.waitErr)
			t.Cleanup(lifecycle.releaseWait)
			a := &App{
				Infra:        &Infra{logger: zap.NewNop()},
				Repositories: &Repositories{},
				Services:     &Services{},
				workspaceV2:  lifecycle,
				shutdownCh:   make(chan struct{}),
			}

			const callers = 100
			results := make(chan error, callers)
			go func() { results <- a.Shutdown(context.Background()) }()
			select {
			case <-lifecycle.started:
			case <-time.After(time.Second):
				t.Fatal("first shutdown did not reach workspace wait")
			}
			for range callers - 1 {
				go func() { results <- a.Shutdown(context.Background()) }()
			}
			select {
			case err := <-results:
				lifecycle.releaseWait()
				t.Fatalf("shutdown caller returned before the shared execution completed: %v", err)
			case <-time.After(25 * time.Millisecond):
			}

			lifecycle.releaseWait()
			var first error
			for index := range callers {
				select {
				case err := <-results:
					if index == 0 {
						first = err
					} else {
						require.True(t, err == first, "caller %d received a different result instance", index)
					}
				case <-time.After(time.Second):
					t.Fatalf("shutdown caller %d did not receive the shared result", index)
				}
			}
			for index := range callers {
				require.True(t, a.Shutdown(context.Background()) == first, "repeat caller %d received a different result", index)
			}
			closeCalls, waitCalls := lifecycle.calls()
			require.Equal(t, 1, closeCalls)
			require.Equal(t, 1, waitCalls)
			if test.waitErr == nil {
				require.NoError(t, first)
				return
			}
			require.ErrorIs(t, first, test.waitErr)
			var typedErr *shutdownRegressionError
			require.True(t, errors.As(first, &typedErr))
			require.Same(t, test.waitErr, typedErr)
		})
	}
}
