const { CommandCategory } = require("@src/structures");
const { EMBED_COLORS } = require("@root/config.js");
const Logger = require("@helpers/Logger");
const { EmbedBuilder } = require("discord.js");
const moment = require("moment-timezone");
const axios = require("axios");

const gameInfoDataStore = require("@helpers/GameInfoDataStore");

const API_URL = "https://lodestonenews.com/news/maintenance/current";

module.exports = {
  name: "maint",
  description: "Shows the FFXIV maintenance information",
  category: "GAMEINFO",
  botPermissions: ["EmbedLinks"],
  command: { enabled: false, usage: "[command]" },
  slashCommand: { enabled: true, options: [] },

  async interactionRun(interaction) {
    try {
      const embed = await getMaintenanceEmbed();
      await interaction.followUp({ embeds: [embed ?? createErrorEmbed()] });
    } catch (err) {
      Logger.error("maint interactionRun", err);
      await interaction.followUp("Error loading maintenance information.");
    }
  },
};

async function getMaintenanceEmbed() {
  const maintInfo = await getMaintInfo();
  if (!isValidMaintInfo(maintInfo)) return null;

  const { start_stamp, end_stamp, title_kr, url } = maintInfo;

  return new EmbedBuilder()
    .setTitle(title_kr)
    .setURL(url)
    .addFields(
      { name: "Start time", value: `<t:${start_stamp}:F>`, inline: false },
      { name: "End time", value: `<t:${end_stamp}:F>`, inline: false },
      { name: "Time remaining", value: `<t:${end_stamp}:R>`, inline: false }
    )
    .setColor(EMBED_COLORS.SUCCESS)
    .setTimestamp()
    .setThumbnail(CommandCategory.GAMEINFO?.image)
    .setFooter({ text: "From Lodestone News" });
}

async function getMaintInfo() {
  const maintData = await getMaintData();
  return maintData?.MAINTINFO ?? null;
}

async function getMaintData() {
  const now = moment().unix();

  let gameItems;
  try {
    const { data } = await axios.get(API_URL);
    gameItems = data.game;
  } catch (e) {
    Logger.error("maint API fetch", e);
    return loadIfValid(now);
  }

  if (!Array.isArray(gameItems) || gameItems.length === 0) {
    return loadIfValid(now);
  }

  // Keep existing cache when it is still valid (same id and not expired).
  const saved = await loadData();
  const savedInfo = saved.MAINTINFO;
  if (savedInfo && savedInfo.id === gameItems[0].id && savedInfo.end_stamp > now) {
    return saved;
  }

  const item = gameItems[0];
  if (!isValidMaintItem(item)) return loadIfValid(now);

  const startMoment = moment(item.start);
  const endMoment = moment(item.end);
  if (!startMoment.isValid() || !endMoment.isValid()) return loadIfValid(now);

  const startMs = startMoment.valueOf();
  const endMs = endMoment.valueOf();
  const startStamp = Math.floor(startMs / 1000);
  const endStamp = Math.floor(endMs / 1000);

  const titleKr = formatKoreanTitle(item.start, item.end);

  const newData = {
    MAINTINFO: {
      id: item.id,
      start_stamp: startStamp,
      end_stamp: endStamp,
      title_kr: titleKr,
      url: item.url,
    },
  };
  await saveData(newData);
  return newData;
}

async function loadIfValid(now) {
  const saved = await loadData();
  if (isValidMaintInfo(saved.MAINTINFO) && saved.MAINTINFO.end_stamp > now) return saved;
  return null;
}

async function loadData() {
  return gameInfoDataStore.load();
}

async function saveData(obj) {
  try {
    await gameInfoDataStore.update(obj);
  } catch (e) {
    Logger.error("maint saveData", e);
  }
}

function isValidMaintItem(item) {
  return (
    item &&
    typeof item.id === "string" &&
    typeof item.start === "string" &&
    typeof item.end === "string" &&
    typeof item.url === "string"
  );
}

function isValidMaintInfo(info) {
  return (
    info &&
    typeof info.id === "string" &&
    Number.isFinite(info.start_stamp) &&
    Number.isFinite(info.end_stamp) &&
    typeof info.title_kr === "string" &&
    typeof info.url === "string"
  );
}

function formatKoreanTitle(startISO, endISO) {
  const s = moment(startISO).tz("Asia/Tokyo");
  const e = moment(endISO).tz("Asia/Tokyo");

  const sM = s.month() + 1;
  const sD = s.date();
  const eM = e.month() + 1;
  const eD = e.date();

  let range;
  if (sM === eM) {
    if (sD === eD) {
      // Same day: M/D
      range = `${sM}/${sD}`;
    } else {
      // Same month, different day: M/D-D
      range = `${sM}/${sD}-${eD}`;
    }
  } else {
    // Different month: M/D - M/D
    range = `${sM}/${sD} - ${eM}/${eD}`;
  }

  return `Global maintenance window (${range})`;
}

function createErrorEmbed() {
  return new EmbedBuilder()
    .setTitle("Cannot load maintenance data")
    .setDescription("No maintenance information is currently available.")
    .setURL("https://jp.finalfantasyxiv.com/lodestone")
    .setColor(EMBED_COLORS.ERROR)
    .setThumbnail(CommandCategory.GAMEINFO.image)
    .setFooter({ text: "From Lodestone News" });
}
