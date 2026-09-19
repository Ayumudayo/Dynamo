/*
 * Browser-safe mutation transport. This deliberately has no DOM dependency so
 * its outcome contract can be exercised with node --test.
 */
(function exposeDashboardMutationTransport(root) {
  const DEFAULT_DEADLINE_MS = 15000;

  function messageFromBody(body, fallback) {
    return body && typeof body.message === 'string' && body.message.trim()
      ? body.message
      : fallback;
  }

  async function parseJson(response) {
    try {
      return { ok: true, body: JSON.parse(await response.text()) };
    } catch (_) {
      return { ok: false, body: null };
    }
  }

  async function request(fetchImpl, url, init = {}, options = {}) {
    const deadlineMs = options.deadlineMs ?? DEFAULT_DEADLINE_MS;
    const controller = options.controller ?? new AbortController();
    let timedOut = false;
    const timer = setTimeout(() => {
      timedOut = true;
      controller.abort();
    }, deadlineMs);

    try {
      const response = await fetchImpl(url, { ...init, signal: controller.signal });
      const parsed = await parseJson(response);
      if (!response.ok) {
        return {
          kind: 'definite-failure',
          status: response.status,
          message: messageFromBody(parsed.body, `Request failed (${response.status}).`),
        };
      }
      if (!parsed.ok) {
        return {
          kind: 'unknown',
          reason: 'invalid-response',
          message: 'The server response could not be confirmed. The outcome is unknown.',
        };
      }
      return { kind: 'success', status: response.status, body: parsed.body };
    } catch (error) {
      return {
        kind: 'unknown',
        reason: timedOut || error?.name === 'AbortError' ? 'timeout' : 'network',
        message: timedOut || error?.name === 'AbortError'
          ? 'The request timed out. The outcome is unknown.'
          : 'The network request did not finish. The outcome is unknown.',
      };
    } finally {
      clearTimeout(timer);
    }
  }

  const api = Object.freeze({ DEFAULT_DEADLINE_MS, request });
  root.DynamoMutationTransport = api;
  if (typeof module !== 'undefined' && module.exports) module.exports = api;
}(globalThis));
