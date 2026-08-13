/* GENERATED FILE — do not edit by hand. Source: contracts/control-plane/src/policy.json */
export const contractVersion = "control-api/1" as const;

export const challengePolicy = {
  "secretStorage": "hash_only",
  "purposes": [
    "verify_email",
    "reset_password",
    "login_email_otp",
    "change_email",
    "high_risk_action"
  ],
  "emailCode": {
    "length": 6,
    "charset": "digits",
    "ttlSeconds": 600,
    "maxAttempts": 5,
    "resendAfterSeconds": 60
  },
  "consume": {
    "atomic": true,
    "replayAllowed": false
  },
  "errors": {
    "expired": "challenge_expired",
    "exhausted": "challenge_attempts_exhausted",
    "consumed": "challenge_already_consumed",
    "rateLimited": "challenge_rate_limited",
    "invalid": "challenge_invalid",
    "purposeMismatch": "challenge_purpose_mismatch"
  }
} as const;

export const sessionPolicy = {
  "accessTokenTtlSeconds": 600,
  "refreshTokenTtlSeconds": 2592000,
  "refreshStorage": "server_hash_only",
  "refreshRotation": true,
  "reuseDetection": {
    "action": "revoke_session_family"
  },
  "revoke": {
    "singleDevice": true,
    "allDevices": true
  }
} as const;

export const tokenBoundary = {
  "productSession": {
    "issuer": "https://control.fns.local/product",
    "audience": "fns-product-session",
    "scopes": [
      "product.session"
    ],
    "lifetimeSeconds": 600,
    "storage": "memory_or_secure_session",
    "verificationEntry": "control-plane/session"
  },
  "agentToken": {
    "issuer": "https://control.fns.local/agent",
    "audience": "fns-workspace-agent",
    "scopes": [
      "workspace.agent",
      "workspace.sync"
    ],
    "lifetimeSeconds": 900,
    "storage": "workspace_node_credential",
    "verificationEntry": "data-plane/agent-token"
  },
  "interchangeable": false,
  "productSessionAcceptedAsAgentToken": false
} as const;

export const authErrors = {
  "antiEnumeration": true,
  "loginFailureCode": "auth_invalid_credentials",
  "registerConflictCode": "auth_registration_accepted",
  "forgotPasswordAcceptedCode": "auth_recovery_accepted",
  "sharedMessage": "If an account exists for this email, further instructions were sent or credentials were checked.",
  "timingSafe": true
} as const;

export const redaction = {
  "neverLog": [
    "password",
    "email_code",
    "verification_code",
    "access_token",
    "refresh_token",
    "agent_token",
    "authorization",
    "secret",
    "raw_challenge_secret"
  ]
} as const;
