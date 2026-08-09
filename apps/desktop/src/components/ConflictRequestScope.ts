export interface ConflictControlIdentity {
  requestId: string;
  projectGeneration: string;
}

export interface ConflictRequestScopeCleanup {
  projectGeneration: string;
  activeRequestIds: string[];
}

type UuidFactory = () => string;

function browserUuid() {
  return crypto.randomUUID();
}

export class ConflictRequestScope {
  readonly projectGeneration: string;

  private active = true;
  private refreshInFlight = false;
  private refreshPending = false;
  private resolution:
    | { conflictKey: string; identity: ConflictControlIdentity }
    | undefined;
  private readonly uuid: UuidFactory;

  constructor(uuid: UuidFactory = browserUuid) {
    this.uuid = uuid;
    this.projectGeneration = uuid();
  }

  isActive() {
    return this.active;
  }

  beginRefresh() {
    if (!this.active) return false;
    if (this.refreshInFlight) {
      this.refreshPending = true;
      return false;
    }
    this.refreshInFlight = true;
    return true;
  }

  finishRefresh() {
    this.refreshInFlight = false;
    if (!this.active) {
      this.refreshPending = false;
      return false;
    }
    const refreshPending = this.refreshPending;
    this.refreshPending = false;
    return refreshPending;
  }

  beginResolution(conflictKey: string): ConflictControlIdentity | null {
    if (!this.active || this.resolution) return null;
    const identity = {
      requestId: this.uuid(),
      projectGeneration: this.projectGeneration,
    };
    this.resolution = { conflictKey, identity };
    return identity;
  }

  acceptsResolution(identity: ConflictControlIdentity) {
    return (
      this.active &&
      this.resolution?.identity.requestId === identity.requestId &&
      this.resolution.identity.projectGeneration === identity.projectGeneration
    );
  }

  finishResolution(identity: ConflictControlIdentity) {
    if (
      this.resolution?.identity.requestId === identity.requestId &&
      this.resolution.identity.projectGeneration === identity.projectGeneration
    ) {
      this.resolution = undefined;
    }
  }

  deactivate(): ConflictRequestScopeCleanup {
    const activeRequestIds = this.resolution
      ? [this.resolution.identity.requestId]
      : [];
    this.active = false;
    this.resolution = undefined;
    this.refreshInFlight = false;
    this.refreshPending = false;
    return {
      projectGeneration: this.projectGeneration,
      activeRequestIds,
    };
  }
}
