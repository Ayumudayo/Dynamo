const { STOCK, EMBED_COLORS } = require("@root/config.js");
const { CommandCategory } = require("@src/structures");
const { EmbedBuilder } = require("discord.js");
const YahooFinance = require("yahoo-finance2").default;

const yahooFinance = new YahooFinance();

const DEFAULT_SYMBOL = "NVDA";
const UP_EMOJI = "<:yangbonghoro:1162456430360662018>";
const DOWN_EMOJI = "<:sale:1162457546532073623>";
const QUOTE_FIELDS = [
  "symbol",
  "shortName",
  "longName",
  "currency",
  "marketState",
  "regularMarketPrice",
  "regularMarketChange",
  "regularMarketChangePercent",
  "preMarketPrice",
  "preMarketChange",
  "preMarketChangePercent",
  "postMarketPrice",
  "postMarketChange",
  "postMarketChangePercent",
  "regularMarketDayHigh",
  "regularMarketDayLow",
  "regularMarketVolume",
];

function normalizeSymbol(symbol) {
  if (typeof symbol !== "string") return DEFAULT_SYMBOL;

  const trimmed = symbol.trim().toUpperCase();
  return trimmed || DEFAULT_SYMBOL;
}

function normalizeSymbols(symbols) {
  if (!Array.isArray(symbols)) return [];

  return [...new Set(symbols.map(normalizeSymbol).filter(Boolean))];
}

function getTotalUpdates() {
  return Math.max(1, Math.floor(STOCK.MAX_REFRESH_TIME / STOCK.REFRESH_INTERVAL));
}

function normalizeMarketState(rawState) {
  switch (rawState) {
    case "REGULAR":
      return "Regular Market";
    case "PRE":
    case "PREPRE":
      return "Pre Market";
    case "POST":
    case "POSTPOST":
      return "Post Market";
    case "CLOSED":
      return "Closed";
    default:
      return "Unknown";
  }
}

function getMarketStatusEmoji(phase) {
  switch (phase) {
    case "Regular Market":
      return ":green_circle:";
    case "Pre Market":
      return ":orange_circle:";
    case "Post Market":
    case "Closed":
      return ":red_circle:";
    default:
      return ":black_circle:";
  }
}

function getStopReasonForPhase(phase) {
  switch (phase) {
    case "Regular Market":
    case "Pre Market":
      return null;
    case "Post Market":
      return "post_market";
    case "Closed":
      return "market_closed";
    default:
      return "market_state_unknown";
  }
}

function safeNumber(value) {
  return typeof value === "number" && Number.isFinite(value) ? value : null;
}

function normalizePercent(value, { inputIsPercent = false } = {}) {
  const number = safeNumber(value);
  if (number === null) return null;
  return inputIsPercent ? number / 100 : number;
}

function buildFooter(updateCount, totalUpdates) {
  return `Data from Yahoo Finance. Update ${updateCount}/${totalUpdates}.`;
}

function formatMoney(label, value) {
  if (value === null) return "N/A";
  if (!label) return value.toFixed(2);
  if (/^[A-Z]{3}$/.test(label)) return `${label} ${value.toFixed(2)}`;
  return `${label}${value.toFixed(2)}`;
}

function formatChange(change, changePercent) {
  if (change === null || changePercent === null) return "N/A";

  const emoji = change > 0 ? UP_EMOJI : change < 0 ? DOWN_EMOJI : "";
  const suffix = emoji ? ` ${emoji}` : "";

  return `${change.toFixed(2)} (${(changePercent * 100).toFixed(2)}%)${suffix}`;
}

function formatVolume(volume) {
  return volume === null ? "N/A" : volume.toLocaleString("en-US");
}

function getDisplayName(snapshot) {
  return snapshot.longName || snapshot.shortName || snapshot.symbol;
}

function getQuoteUrl(symbol) {
  return `https://finance.yahoo.com/quote/${encodeURIComponent(symbol)}`;
}

function getCurrentMarketData(snapshot, phase = snapshot.phase) {
  if (phase === "Pre Market" && snapshot.preMarketPrice !== null) {
    return {
      price: snapshot.preMarketPrice,
      change: snapshot.preMarketChange,
      changePercent: snapshot.preMarketChangePercent,
    };
  }

  if (phase === "Post Market" && snapshot.postMarketPrice !== null) {
    return {
      price: snapshot.postMarketPrice,
      change: snapshot.postMarketChange,
      changePercent: snapshot.postMarketChangePercent,
    };
  }

  return {
    price: snapshot.regularMarketPrice,
    change: snapshot.regularMarketChange,
    changePercent: snapshot.regularMarketChangePercent,
  };
}

function getEmbedColor(snapshot) {
  const current = getCurrentMarketData(snapshot);

  if (current.change > 0) return STOCK.UPWARD_EMBED;
  if (current.change < 0) return STOCK.DOWNWARD_EMBED;
  return STOCK.DEFAULT_EMBED;
}

function normalizeSummaryPrice(price) {
  if (!price?.symbol) return null;

  return {
    symbol: price.symbol,
    shortName: price.shortName || null,
    longName: price.longName || null,
    currencyLabel: price.currencySymbol || price.currency || "",
    phase: normalizeMarketState(price.marketState),
    regularMarketPrice: safeNumber(price.regularMarketPrice),
    regularMarketChange: safeNumber(price.regularMarketChange),
    regularMarketChangePercent: normalizePercent(price.regularMarketChangePercent),
    preMarketPrice: safeNumber(price.preMarketPrice),
    preMarketChange: safeNumber(price.preMarketChange),
    preMarketChangePercent: normalizePercent(price.preMarketChangePercent),
    postMarketPrice: safeNumber(price.postMarketPrice),
    postMarketChange: safeNumber(price.postMarketChange),
    postMarketChangePercent: normalizePercent(price.postMarketChangePercent),
    regularMarketDayHigh: safeNumber(price.regularMarketDayHigh),
    regularMarketDayLow: safeNumber(price.regularMarketDayLow),
    regularMarketVolume: safeNumber(price.regularMarketVolume),
  };
}

function normalizeQuote(quote) {
  if (!quote?.symbol) return null;

  return {
    symbol: quote.symbol,
    shortName: quote.shortName || null,
    longName: quote.longName || null,
    currencyLabel: quote.currency || "",
    phase: normalizeMarketState(quote.marketState),
    regularMarketPrice: safeNumber(quote.regularMarketPrice),
    regularMarketChange: safeNumber(quote.regularMarketChange),
    regularMarketChangePercent: normalizePercent(quote.regularMarketChangePercent, {
      inputIsPercent: true,
    }),
    preMarketPrice: safeNumber(quote.preMarketPrice),
    preMarketChange: safeNumber(quote.preMarketChange),
    preMarketChangePercent: normalizePercent(quote.preMarketChangePercent, {
      inputIsPercent: true,
    }),
    postMarketPrice: safeNumber(quote.postMarketPrice),
    postMarketChange: safeNumber(quote.postMarketChange),
    postMarketChangePercent: normalizePercent(quote.postMarketChangePercent, {
      inputIsPercent: true,
    }),
    regularMarketDayHigh: safeNumber(quote.regularMarketDayHigh),
    regularMarketDayLow: safeNumber(quote.regularMarketDayLow),
    regularMarketVolume: safeNumber(quote.regularMarketVolume),
  };
}

async function fetchSingleStockSnapshot(symbol) {
  const normalizedSymbol = normalizeSymbol(symbol);
  const quoteSummary = await yahooFinance.quoteSummary(normalizedSymbol, { modules: ["price"] });
  return normalizeSummaryPrice(quoteSummary?.price);
}

async function fetchBatchStockSnapshots(symbols) {
  const normalizedSymbols = normalizeSymbols(symbols);
  if (!normalizedSymbols.length) return [];

  const quotes = await yahooFinance.quote(normalizedSymbols, {
    fields: QUOTE_FIELDS,
    return: "object",
  });

  return normalizedSymbols.map((symbol) => {
    const snapshot = normalizeQuote(quotes?.[symbol]);

    if (snapshot) return snapshot;

    return {
      symbol,
      errorMessage: "Invalid Ticker",
    };
  });
}

function getRepresentativePhase(snapshots) {
  const validSnapshots = snapshots.filter((snapshot) => !snapshot.errorMessage);
  if (validSnapshots.some((snapshot) => snapshot.phase === "Regular Market")) return "Regular Market";
  if (validSnapshots.some((snapshot) => snapshot.phase === "Pre Market")) return "Pre Market";
  if (validSnapshots.some((snapshot) => snapshot.phase === "Post Market")) return "Post Market";
  if (validSnapshots.some((snapshot) => snapshot.phase === "Closed")) return "Closed";
  if (validSnapshots.length > 0) return validSnapshots[0].phase;
  return "Unknown";
}

function buildSingleStockEmbed(snapshot, { updateCount, totalUpdates }) {
  const phaseEmoji = getMarketStatusEmoji(snapshot.phase);

  const embed = new EmbedBuilder()
    .setTitle(`${getDisplayName(snapshot)} / [${snapshot.symbol}]`)
    .setURL(getQuoteUrl(snapshot.symbol))
    .setThumbnail(CommandCategory.STOCK?.image)
    .setColor(getEmbedColor(snapshot))
    .setFooter({ text: buildFooter(updateCount, totalUpdates) })
    .setTimestamp(Date.now())
    .addFields(
      { name: "Market State", value: `${snapshot.phase} ${phaseEmoji}`, inline: false },
      { name: "Price", value: formatMoney(snapshot.currencyLabel, snapshot.regularMarketPrice), inline: true },
      {
        name: "Change",
        value: formatChange(snapshot.regularMarketChange, snapshot.regularMarketChangePercent),
        inline: true,
      },
      { name: " ", value: " ", inline: false }
    );

  if (snapshot.phase === "Pre Market") {
    embed.addFields(
      { name: "Pre - Price", value: formatMoney(snapshot.currencyLabel, snapshot.preMarketPrice), inline: true },
      {
        name: "Pre - Change",
        value: formatChange(snapshot.preMarketChange, snapshot.preMarketChangePercent),
        inline: true,
      },
      { name: " ", value: " ", inline: false }
    );
  } else if (snapshot.phase === "Post Market") {
    embed.addFields(
      { name: "Post - Price", value: formatMoney(snapshot.currencyLabel, snapshot.postMarketPrice), inline: true },
      {
        name: "Post - Change",
        value: formatChange(snapshot.postMarketChange, snapshot.postMarketChangePercent),
        inline: true,
      },
      { name: " ", value: " ", inline: false }
    );
  }

  embed.addFields(
    { name: "Day High", value: formatMoney(snapshot.currencyLabel, snapshot.regularMarketDayHigh), inline: true },
    { name: "Day Low", value: formatMoney(snapshot.currencyLabel, snapshot.regularMarketDayLow), inline: true },
    { name: "Volume", value: formatVolume(snapshot.regularMarketVolume), inline: true }
  );

  return embed;
}

function buildEtfEmbed(snapshots, phase, { updateCount, totalUpdates }) {
  const phaseEmoji = getMarketStatusEmoji(phase);

  const embed = new EmbedBuilder()
    .setTitle("ETFs")
    .setThumbnail(CommandCategory.STOCK?.image)
    .setColor(EMBED_COLORS.BOT_EMBED)
    .setFooter({ text: buildFooter(updateCount, totalUpdates) })
    .setTimestamp(Date.now())
    .addFields({ name: "Market State", value: `${phase} ${phaseEmoji}`, inline: false });

  snapshots.forEach((snapshot) => {
    if (snapshot.errorMessage) {
      embed.addFields({ name: snapshot.symbol, value: snapshot.errorMessage, inline: false });
      return;
    }

    const current = getCurrentMarketData(snapshot, phase);

    embed.addFields(
      {
        name: snapshot.symbol,
        value: formatMoney(snapshot.currencyLabel, current.price),
        inline: true,
      },
      {
        name: "Change",
        value: formatChange(current.change, current.changePercent),
        inline: true,
      },
      { name: " ", value: " ", inline: false }
    );
  });

  return embed;
}

async function getSingleStockResponse(symbol, context) {
  const snapshot = await fetchSingleStockSnapshot(symbol);
  if (!snapshot) return null;

  const stopReason = getStopReasonForPhase(snapshot.phase);

  return {
    embed: buildSingleStockEmbed(snapshot, context),
    phase: snapshot.phase,
    stopReason,
    symbol: snapshot.symbol,
  };
}

async function getEtfResponse(symbols, context) {
  const snapshots = await fetchBatchStockSnapshots(symbols);
  const phase = getRepresentativePhase(snapshots);
  const stopReason = getStopReasonForPhase(phase);

  return {
    embed: buildEtfEmbed(snapshots, phase, context),
    phase,
    stopReason,
    snapshots,
  };
}

module.exports = {
  DEFAULT_SYMBOL,
  getEtfResponse,
  getSingleStockResponse,
  getTotalUpdates,
  normalizeSymbol,
  normalizeSymbols,
};
