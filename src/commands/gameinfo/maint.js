const { CommandCategory } = require("@src/structures");
const { EMBED_COLORS } = require("@root/config.js");
const { EmbedBuilder } = require("discord.js");
const fs = require("fs/promises");
const path = require("path");
const moment = require("moment-timezone");
const axios = require("axios");

const dataFilePath = path.join(__dirname, "../../data.json");
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
      console.error(err);
      await interaction.followUp("점검 정보를 불러오는 중 오류가 발생했습니다.");
    }
  },
};

async function getMaintenanceEmbed() {
  const maintData = await getMaintData();
  if (!maintData) return null;

  const { start_stamp, end_stamp, title_kr, url } = maintData.MAINTINFO;
  return new EmbedBuilder()
    .setTitle(title_kr)
    .setURL(url)
    .addFields(
      { name: "시작 시각",      value: `<t:${start_stamp}:F>`, inline: false },
      { name: "종료 시각",      value: `<t:${end_stamp}:F>`, inline: false },
      { name: "종료까지 남은 시간", value: `<t:${end_stamp}:R>`, inline: false }
    )
    .setColor(EMBED_COLORS.SUCCESS)
    .setTimestamp()
    .setThumbnail(CommandCategory.GAMEINFO?.image)
    .setFooter({ text: "From Lodestone News" });
}

async function getMaintData() {
  const now = moment().unix();

  let gameItems;
  try {
    const { data } = await axios.get(API_URL);
    gameItems = data.game;
  } catch (e) {
    console.error("API fetch error:", e);
    return loadIfValid(now);
  }

  if (!Array.isArray(gameItems) || gameItems.length === 0) {
    return loadIfValid(now);
  }

  // 캐시된 데이터가 아직 유효한지 확인 (ID 동일 & 종료 시간 지나지 않음)
  const saved = await loadData();
  const savedInfo = saved.MAINTINFO;
  if (savedInfo
    && savedInfo.id === gameItems[0].id
    && savedInfo.end_stamp > now
  ) {
    return saved;
  }

  const item = gameItems[0];
  const startMs = moment(item.start).valueOf();
  const endMs   = moment(item.end).valueOf();
  const startStamp = Math.floor(startMs / 1000);
  const endStamp   = Math.floor(endMs   / 1000);

  const titleKr = formatKoreanTitle(item.start, item.end);

  const newData = {
    MAINTINFO: {
      id:         item.id,
      start_stamp: startStamp,
      end_stamp:   endStamp,
      title_kr:    titleKr,
      url:         item.url
    }
  };
  await saveData(newData);
  return newData;
}

async function loadIfValid(now) {
  const saved = await loadData();
  if (saved.MAINTINFO?.end_stamp > now) return saved;
  return null;
}

async function loadData() {
  try {
    const text = await fs.readFile(dataFilePath, "utf-8");
    return JSON.parse(text);
  } catch {
    return {};
  }
}

async function saveData(obj) {
  try {
    const existing = await loadData();
    const merged = { ...existing, ...obj };
    await fs.writeFile(dataFilePath, JSON.stringify(merged, null, 2));
  } catch (e) {
    console.error("Error saving data:", e);
  }
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
      // 같은 날: M/D
      range = `${sM}/${sD}`;
    } else {
      // 같은 달, 다른 날: M/D-D
      range = `${sM}/${sD}-${eD}`;
    }
  } else {
    // 다른 달: M/D - M/D
    range = `${sM}/${sD} - ${eM}/${eD}`;
  }

  return `전 월드 유지보수 작업 (${range})`;
}

function createErrorEmbed() {
  return new EmbedBuilder()
    .setTitle("점검 정보를 불러올 수 없습니다")
    .setDescription("현재 점검 공지가 없거나 API 업데이트가 되지 않았습니다.")
    .setURL("https://jp.finalfantasyxiv.com/lodestone")
    .setColor(EMBED_COLORS.ERROR)
    .setThumbnail(CommandCategory.GAMEINFO.image)
    .setFooter({ text: "From Lodestone News" });
}