const { ActionRowBuilder, ButtonBuilder, ButtonStyle } = require("discord.js");

const { STOCK } = require("@root/config.js");
const { startSerializedRefresh } = require("@helpers/StockPoller");
const { getEtfResponse, getSingleStockResponse, getTotalUpdates } = require("@helpers/StockService");

const STOCK_REFRESH_BUTTON_ID = "STOCK_REFRESH";
const MAX_STORED_SESSIONS = 200;
const sessions = new Map();

function createStockSession({ symbol }) {
  return createSession({ kind: "stock", symbol });
}

function createEtfSession({ tickers }) {
  return createSession({ kind: "etf", tickers });
}

function createSession({ kind, symbol = null, tickers = [] }) {
  return {
    active: false,
    controller: null,
    kind,
    lastStopReason: null,
    manualRestartInProgress: false,
    manualRefreshCount: 0,
    messageId: null,
    symbol,
    tickers,
    totalUpdates: getTotalUpdates(),
  };
}

function buildRefreshComponents() {
  return [
    new ActionRowBuilder().addComponents(
      new ButtonBuilder()
        .setCustomId(STOCK_REFRESH_BUTTON_ID)
        .setLabel("Refresh")
        .setEmoji("🔄")
        .setStyle(ButtonStyle.Secondary)
    ),
  ];
}

function getSession(messageId) {
  return sessions.get(messageId) || null;
}

function canStartManualRefresh(session) {
  return session.manualRefreshCount < STOCK.MAX_MANUAL_REFRESHES;
}

async function sendInitialSessionReply(interaction, session) {
  const response = await fetchInitialResponse(session);
  if (!response?.embed) return { response: null, session };

  const message = await interaction.editReply(buildPayload(response));
  registerSession(message, session);

  if (!response.stopReason) {
    startSessionLoop(message, session);
  } else {
    markSessionStopped(session, response.stopReason);
  }

  return { message, response, session };
}

async function restartSessionFromButton(interaction, session) {
  session.active = true;
  session.lastStopReason = null;
  session.controller = null;

  try {
    const response = await fetchInitialResponse(session);
    if (!response?.embed) {
      markSessionStopped(session, "fetch_failed");
      return null;
    }

    session.manualRefreshCount += 1;
    await interaction.message.edit(buildPayload(response));

    if (!response.stopReason) {
      startSessionLoop(interaction.message, session);
    } else {
      markSessionStopped(session, response.stopReason);
    }

    return response;
  } catch (error) {
    markSessionStopped(session, "restart_failed");
    throw error;
  }
}

function startSessionLoop(message, session) {
  session.active = true;
  session.lastStopReason = null;
  session.totalUpdates = getTotalUpdates();

  session.controller = startSerializedRefresh({
    buildPayload,
    editMessage: (payload) => message.edit(payload),
    fetchUpdate: createFetchUpdate(session),
    intervalMs: STOCK.REFRESH_INTERVAL,
    loggerScope: getLoggerScope(session),
    maxUpdates: session.totalUpdates,
    onStop: (reason) => {
      markSessionStopped(session, reason);

      if (reason === "interaction_edit_failed") {
        removeSession(session.messageId);
      }
    },
  });

  return session.controller;
}

function registerSession(message, session) {
  pruneSessions();
  session.messageId = message.id;
  sessions.set(message.id, session);
  return session;
}

function removeSession(messageId) {
  sessions.delete(messageId);
}

function createFetchUpdate(session) {
  if (session.kind === "stock") {
    return ({ updateCount, maxUpdates }) =>
      getSingleStockResponse(session.symbol, {
        updateCount,
        totalUpdates: maxUpdates,
      });
  }

  return ({ updateCount, maxUpdates }) =>
    getEtfResponse(session.tickers, {
      updateCount,
      totalUpdates: maxUpdates,
    });
}

async function fetchInitialResponse(session) {
  const context = {
    updateCount: 0,
    totalUpdates: session.totalUpdates,
  };

  if (session.kind === "stock") {
    return getSingleStockResponse(session.symbol, context);
  }

  return getEtfResponse(session.tickers, context);
}

function buildPayload(response) {
  return {
    embeds: [response.embed],
    components: buildRefreshComponents(),
  };
}

function getLoggerScope(session) {
  if (session.kind === "stock") {
    return `stock:${session.symbol}`;
  }

  return `etf:${session.tickers.join(",")}`;
}

function markSessionStopped(session, reason) {
  session.active = false;
  session.controller = null;
  session.lastStopReason = reason;
}

function pruneSessions() {
  while (sessions.size >= MAX_STORED_SESSIONS) {
    const oldestKey = sessions.keys().next().value;
    if (!oldestKey) break;
    sessions.delete(oldestKey);
  }
}

module.exports = {
  STOCK_REFRESH_BUTTON_ID,
  canStartManualRefresh,
  createEtfSession,
  createStockSession,
  getSession,
  restartSessionFromButton,
  sendInitialSessionReply,
};
