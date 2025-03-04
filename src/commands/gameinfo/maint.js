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
  const now = Date.now() / 1000;

  try {
    // 피드 파싱
    const feed = await parser.parseURL(feedUrl);

    // 변경 공지 항목 확인
    const updateNotification = feed.items.find((item) => item.title?.includes("終了時間変更のお知らせ"));
    if (updateNotification) {
      const maintenanceInfo = extractMaintenanceInfo(updateNotification);
      if (maintenanceInfo && maintenanceInfo.end_stamp > now) {
        const translatedTitle = await getTranslation(updateNotification.title);
        const newData = {
          MAINTINFO: {
            ...maintenanceInfo,
            title_kr: translatedTitle,
            url: updateNotification.link,
          },
        };
        await saveData(newData);
        return newData;
      }
    }

    // 변경 공지가 없으면 캐시된 데이터가 유효한지 확인
    const savedData = await loadData();
    if (savedData.MAINTINFO?.end_stamp > now) {
      return savedData;
    }

    // 캐시가 없거나 만료된 경우, 기본 유지보수 공지 사용
    const defaultItem = feed.items.find((item) => item.title?.startsWith("全ワールド"));
    if (!defaultItem) return null;

    const maintenanceInfo = extractMaintenanceInfo(defaultItem);
    if (maintenanceInfo && maintenanceInfo.end_stamp > now) {
      const translatedTitle = await getTranslation(defaultItem.title);
      const newData = {
        MAINTINFO: {
          ...maintenanceInfo,
          title_kr: translatedTitle,
          url: defaultItem.link,
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
