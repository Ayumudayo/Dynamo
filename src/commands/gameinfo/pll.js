const { CommandCategory } = require("@src/structures");
const { EMBED_COLORS } = require("@root/config.js");
const Logger = require("@helpers/Logger");
const { EmbedBuilder } = require("discord.js");
const Parser = require("rss-parser");
const cheerio = require("cheerio");
const moment = require("moment-timezone");

const parser = new Parser();
const gameInfoDataStore = require("@helpers/GameInfoDataStore");
const CACHE_DURATION = 12 * 60 * 60; // 12 hours (seconds)
const PLL_TITLE_REGEX = /第\d+回\s?FFXIV\s?PLL/;
const DATE_REGEX = /(\d{4}年\d{1,2}月\d{1,2}日（[^）]+）)\s?(\d{1,2}:\d{2})頃?～/;
const ROUND_REGEX = /第(\d+)回/;

module.exports = {
  name: "pll",
  description: "Shows the FFXIV Producer Letter Live info",
  category: "GAMEINFO",
  botPermissions: ["EmbedLinks"],
  command: { enabled: false, usage: "[command]" },
  slashCommand: { enabled: true, options: [] },

  async interactionRun(interaction) {
    try {
      const embed = await getPLLEmbed();
      await interaction.followUp({ embeds: [embed ?? createErrorEmbed()] });
    } catch (err) {
      Logger.error("pll interactionRun", err);
      await interaction.followUp("An error occurred while processing your request.");
    }
  },
};

async function getPLLEmbed() {
  const pllData = await getPLLData();
  if (!pllData) return null;

  const { fixedTitle, start_stamp, url } = pllData;
  const timeValue = start_stamp ? `<t:${start_stamp}:F>` : "Unavailable";
  const relativeTimeValue = start_stamp ? `<t:${start_stamp}:R>` : "Unavailable";

  return new EmbedBuilder()
    .setTitle(fixedTitle)
    .setURL(url)
    .addFields(
      { name: "Broadcast start", value: timeValue, inline: false },
      { name: "Time remaining", value: relativeTimeValue, inline: false }
    )
    .setColor(EMBED_COLORS.SUCCESS)
    .setTimestamp()
    .setThumbnail(CommandCategory.GAMEINFO?.image)
    .setFooter({ text: "From Lodestone News" });
}

async function getPLLData() {
  const now = Date.now() / 1000;

  // Check cache first
  const cachedData = await getCachedData(now);
  if (cachedData) return cachedData;

  try {
    const feed = await parser.parseURL("https://jp.finalfantasyxiv.com/lodestone/news/topics.xml");
    if (!feed?.items?.length) return loadFallbackData(now);

    const targetItem = findPllItem(feed.items);
    if (!targetItem) return loadFallbackData(now);

    const pllInfo = await processPLLItem(targetItem);
    if (!pllInfo || !isValidPllInfo(pllInfo)) return loadFallbackData(now);

    const newData = {
      PLLINFO: {
        ...pllInfo,
        expireTime: Math.floor(now + CACHE_DURATION),
      },
    };

    await saveData(newData);
    return newData.PLLINFO;
  } catch (error) {
    Logger.error("pll fetch", error);
    return loadFallbackData(now);
  }
}

async function processPLLItem(item) {
  const { title, link, summary } = item;
  const summaryHtml = typeof summary === "string" ? summary : "";
  const headingText = extractHeadingText(summaryHtml, title);

  const roundMatch = headingText.match(ROUND_REGEX);
  const roundNumber = roundMatch?.[1] || "";
  const start_stamp = extractStartTime(summaryHtml);
  const fixedTitle = generateFixedTitle(roundNumber, start_stamp);

  return {
    fixedTitle,
    url: link || "https://jp.finalfantasyxiv.com/lodestone",
    start_stamp,
  };
}

function extractStartTime(summary) {
  const summaryText = extractSummaryText(summary);
  const dateMatch = summaryText.match(DATE_REGEX);
  if (!dateMatch) return null;

  const dateStrClean = dateMatch[1].replace(/（[^）]+）/, "");
  const timeString = `${dateStrClean} ${dateMatch[2]}`;
  const parsed = moment.tz(timeString, "YYYY年M月D日 HH:mm", "Asia/Tokyo");

  return parsed.isValid() ? parsed.unix() : null;
}

function generateFixedTitle(roundNumber, start_stamp) {
  if (!start_stamp) {
    return "Round XX Producer Letter Live date to be announced";
  }

  const formattedDate = moment.unix(start_stamp).tz("Asia/Seoul").format("M/D");
  const roundText = roundNumber ? `Round ${roundNumber}` : "Round XX";

  return `${roundText} Producer Letter Live scheduled on ${formattedDate}`;
}

function findPllItem(items) {
  return (
    items.find((item) => PLL_TITLE_REGEX.test(item.title || "")) ||
    items.find((item) => {
      const headingText = extractHeadingText(item.summary || "", item.title || "");
      return PLL_TITLE_REGEX.test(headingText);
    })
  );
}

function extractHeadingText(summary, title) {
  const $ = cheerio.load(summary || "", { decodeEntities: false });

  return (
    $("h3.mdl-title__heading--lg").first().text().trim() ||
    $("h3").first().text().trim() ||
    (typeof title === "string" ? title : "")
  );
}

function extractSummaryText(summary) {
  const $ = cheerio.load(summary || "", { decodeEntities: false });
  return $.text().replace(/\s+/g, " ").trim();
}

async function getCachedData(now) {
  const savedData = await loadData();
  return isValidPllInfo(savedData.PLLINFO) && savedData.PLLINFO.expireTime > now ? savedData.PLLINFO : null;
}

async function loadFallbackData(now) {
  const savedData = await loadData();
  const savedInfo = savedData.PLLINFO;

  if (!isValidPllInfo(savedInfo)) return null;
  if (savedInfo.expireTime > now) return savedInfo;
  if (savedInfo.start_stamp && savedInfo.start_stamp > now) return savedInfo;
  return null;
}

async function loadData() {
  return gameInfoDataStore.load();
}

async function saveData(newData) {
  try {
    await gameInfoDataStore.update(newData);
  } catch (error) {
    Logger.error("pll saveData", error);
  }
}

function isValidPllInfo(info) {
  return (
    info &&
    typeof info.fixedTitle === "string" &&
    typeof info.url === "string" &&
    Number.isFinite(info.expireTime) &&
    (info.start_stamp === null || Number.isFinite(info.start_stamp))
  );
}

function createErrorEmbed() {
  return new EmbedBuilder()
    .setTitle("No PLL Info")
    .setDescription("No producer letter schedule found.")
    .setURL("https://jp.finalfantasyxiv.com/lodestone")
    .setColor(EMBED_COLORS.ERROR)
    .setThumbnail(CommandCategory.GAMEINFO?.image);
}
