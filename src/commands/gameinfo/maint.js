const { CommandCategory } = require("@src/structures");
const { EMBED_COLORS } = require("@root/config.js");
const { EmbedBuilder } = require("discord.js");
const Parser = require("rss-parser");
const fs = require("fs/promises");
const path = require("path");
const moment = require("moment-timezone");
const { translate } = require("@helpers/HttpUtils");

const parser = new Parser();
const dataFilePath = path.join(__dirname, "../../data.json");

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
      await interaction.followUp({ embeds: [embed ?? createErrorEmbed()] }); // 수정된 부분
    } catch (err) {
      console.error(err);
      await interaction.followUp("An error occurred while processing your request.");
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
      { name: "Begin", value: `<t:${start_stamp}:F>`, inline: false },
      { name: "End", value: `<t:${end_stamp}:F>`, inline: false },
      { name: "Until End", value: `<t:${end_stamp}:R>`, inline: false }
    )
    .setColor(EMBED_COLORS.SUCCESS)
    .setTimestamp()
    .setThumbnail(CommandCategory.GAMEINFO?.image);
}

async function getMaintData() {
  const feedUrl = "https://jp.finalfantasyxiv.com/lodestone/news/news.xml";
  const now = Date.now() / 1000; // 수정된 부분

  try {
    const savedData = await loadData();
    if (savedData.MAINTINFO?.end_stamp > now) {
      return savedData;
    }

    const feed = await parser.parseURL(feedUrl);
    const targetItem = feed.items.find((item) => item.title?.startsWith("全ワールド"));
    if (!targetItem) return null;

    const maintenanceInfo = extractMaintenanceInfo(targetItem);
    if (maintenanceInfo && maintenanceInfo.end_stamp > now) {
      const translatedTitle = await getTranslation(targetItem.title);
      const newData = {
        MAINTINFO: {
          ...maintenanceInfo,
          title_kr: translatedTitle,
          url: targetItem.link,
        },
      };
      await saveData(newData);
      return newData;
    }

    return null;
  } catch (error) {
    console.error("Error fetching maintenance data:", error);
    return null;
  }
}

async function getTranslation(input) {
  const data = await translate(input, "ko");
  return data ? data.output : "Failed to translate the title";
}

async function saveData(newData) {
  try {
    const existingData = await loadData();
    await fs.writeFile(dataFilePath, JSON.stringify({ ...existingData, ...newData }, null, 2));
  } catch (error) {
    console.error("Error saving data:", error);
  }
}

async function loadData() {
  try {
    const data = await fs.readFile(dataFilePath, "utf-8");
    return JSON.parse(data);
  } catch (error) {
    return {};
  }
}

function extractMaintenanceInfo(item) {
  const startTimeRegex = /日　時：(\d{4}年\d{1,2}月\d{1,2}日\(.\)) (\d{1,2}:\d{2})より/;
  const endTimeRegex = /(\d{4}年\d{1,2}月\d{1,2}日\(.\))? ?(\d{1,2}:\d{2})頃まで/;
  const startTimeMatch = item.content.match(startTimeRegex);
  const endTimeMatch = item.content.match(endTimeRegex);
  if (!startTimeMatch || !endTimeMatch) return null;

  // 중복되는 moment.tz 호출을 헬퍼 함수로 분리
  const parseDate = (dateStr, timeStr) =>
    moment.tz(`${dateStr} ${timeStr}`, "YYYY年MM月DD日(ddd) HH:mm", "ja", "Asia/Tokyo").unix();

  const startTime = parseDate(startTimeMatch[1], startTimeMatch[2]);
  const endTime = endTimeMatch[1]
    ? parseDate(endTimeMatch[1], endTimeMatch[2])
    : parseDate(startTimeMatch[1], endTimeMatch[2]);

  return { start_stamp: startTime, end_stamp: endTime };
}

function createErrorEmbed() {
  return new EmbedBuilder()
    .setTitle("No Maintenance Info")
    .setDescription("There is no maintenance information available.")
    .setURL("https://jp.finalfantasyxiv.com/lodestone")
    .setColor(EMBED_COLORS.ERROR)
    .setThumbnail(CommandCategory.GAMEINFO?.image);
}
