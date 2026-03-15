const Logger = require("@helpers/Logger");

function startSerializedRefresh({
  editMessage,
  intervalMs,
  maxUpdates,
  fetchUpdate,
  loggerScope,
  maxFailures = 3,
  buildPayload,
  onStop,
}) {
  const state = {
    consecutiveFailures: 0,
    inFlight: false,
    stopped: false,
    timer: null,
    updateCount: 0,
  };

  const getPayload = (nextResponse) => {
    if (typeof buildPayload === "function") return buildPayload(nextResponse, state);
    return { embeds: [nextResponse.embed] };
  };

  const stop = (reason) => {
    if (state.stopped) return false;

    state.stopped = true;
    if (state.timer) clearTimeout(state.timer);
    Logger.debug(`${loggerScope} stopped: ${reason}`);
    if (typeof onStop === "function") {
      onStop(reason, { ...state });
    }
    return true;
  };

  const scheduleNextTick = () => {
    if (state.stopped) return;

    if (state.updateCount >= maxUpdates) {
      stop("max_refresh_reached");
      return;
    }

    state.timer = setTimeout(runTick, intervalMs);
  };

  const runTick = async ({ force = false } = {}) => {
    if (state.stopped || state.inFlight) return false;
    if (state.updateCount >= maxUpdates && !force) {
      stop("max_refresh_reached");
      return false;
    }

    state.inFlight = true;
    state.updateCount += 1;

    try {
      const nextResponse = await fetchUpdate({
        maxUpdates,
        updateCount: state.updateCount,
      });

      if (!nextResponse?.embed) {
        state.consecutiveFailures += 1;
        Logger.warn(`${loggerScope} returned no payload (${state.consecutiveFailures}/${maxFailures})`);

        if (state.consecutiveFailures >= maxFailures) stop("fetch_error_threshold");
        return false;
      }

      state.consecutiveFailures = 0;

      try {
        await editMessage(getPayload(nextResponse));
      } catch (error) {
        Logger.error(`${loggerScope} editReply`, error);
        stop("interaction_edit_failed");
        return false;
      }

      if (nextResponse.stopReason) stop(nextResponse.stopReason);
      return true;
    } catch (error) {
      state.consecutiveFailures += 1;
      Logger.error(`${loggerScope} tick`, error);

      if (state.consecutiveFailures >= maxFailures) stop("fetch_error_threshold");
      return false;
    } finally {
      state.inFlight = false;
      scheduleNextTick();
    }
  };

  scheduleNextTick();

  return {
    getState() {
      return { ...state };
    },

    isActive() {
      return !state.stopped;
    },

    stop,
    refreshNow: (options = {}) => runTick({ ...options, force: true }),
  };
}

module.exports = {
  startSerializedRefresh,
};
