import { test, expect } from '@playwright/test';

/**
 * LLM trace observability front-end E2E. TraceView runs in a real browser against
 * the recorded-shape rows DbTraceSink writes to `llm_traces`, served through the
 * real IPC boundary shape (window.__MOCK_INVOKE__['list_llm_traces']). Verifies
 * the timeline renders both a failed 400 turn and a clean 200, and — the whole
 * point of the feature — that a failed turn's real error response body is one
 * click away. This is the diagnostic payoff: a 0.8s "stream failed: 400" turn is
 * now explainable end-to-end without guessing.
 *
 * Paired with the back-end trace::sink DbTraceSink test (which proves the write
 * path: fire-and-forget spawn_blocking INSERT → readable row carrying session_id
 * + status_code + resp_body), the two together cover write → store → invoke →
 * render → expand without spinning up the Tauri desktop app.
 */
test('TraceView renders a failed turn and reveals its error response body', async ({ page }) => {
  await page.goto('/trace.html');

  // Timeline renders both rows: the 400 (failed) and the 200 (clean).
  await expect(page.getByText('deepseek-v4-flash').first()).toBeVisible();
  await expect(page.getByText('400')).toBeVisible();
  await expect(page.getByText('200')).toBeVisible();
  // Latency + token columns render (812ms for the failed call, 120/45 tok clean).
  await expect(page.getByText('812ms')).toBeVisible();
  // 120/45 tok shows in both the 概要 summary (session totals) and the clean
  // call's row — the Langfuse-style summary is new; assert visibility via first.
  await expect(page.getByText('120/45 tok').first()).toBeVisible();

  // The error response body is hidden until the 400 row expands — then the real
  // 400 reason renders into the DOM (the payload that was previously discarded
  // by `format!("GLM stream failed: {status}")`).
  await expect(page.getByText(/invalid_request_error/)).toHaveCount(0);
  await page.getByText('400').click();
  await expect(page.getByText(/invalid_request_error/)).toBeVisible();
  await expect(page.getByText(/request rejected by provider/)).toBeVisible();

  // A clean 2xx now persists its full response body too — symmetric with the
  // error path. The 2026-06-19 trace observability research found "2xx stores
  // NULL" to be an industry outlier; the successful turn's output ("hello
  // there") is now one click away, rendered as a normal (non-error) response.
  await expect(page.getByText(/hello there/)).toHaveCount(0);
  await page.getByText('200').click();
  await expect(page.getByText(/hello there/)).toBeVisible();
});
