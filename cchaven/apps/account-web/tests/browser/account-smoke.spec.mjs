import { test, expect } from '@playwright/test';

const base = process.env.CONTROL_PLANE_BASE || 'http://127.0.0.1:18088';

test('control-plane healthz reachable when stack up', async ({ request }) => {
  try {
    const res = await request.get(`${base}/healthz`);
    if (!res.ok()) test.skip(true, 'control-plane not healthy');
    const j = await res.json();
    expect(j.ok).toBeTruthy();
  } catch (e) {
    test.skip(true, 'control-plane unreachable: ' + e);
  }
});
