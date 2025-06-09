const { CommandCategory } = require("@src/structures");
const { EMBED_COLORS } = require("@root/config.js");
const { EmbedBuilder } = require("discord.js");
const fs = require("fs/promises");
const path = require("path");
const moment = require("moment-timezone");
const { translate } = require("@helpers/HttpUtils");
const axios = require("axios");

const dataFilePath = path.join(__dirname, "../../data.json");
const API_URL = "https://lodestonenews.com/news/maintenance/current?locale=jp";

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
    .setThumbnail(CommandCategory.GAMEINFO?.image)
    .setFooter({ text: "From Lodestone News" });
}

async function getMaintData() {
  const now = Date.now() / 1000;

  try {
    // API에서 유지보수 정보 가져오기
    const response = await axios.get(API_URL);
    const { game } = response.data;

    if (!game || game.length === 0) {
      return await getCachedData(now);
    }

    // 변경 공지 항목 확인
    const updateNotification = game.find((item) => item.title?.includes("終了時間変更"));
    if (updateNotification) {
      const maintenanceInfo = convertToTimestamps(updateNotification);
      if (maintenanceInfo && maintenanceInfo.end_stamp > now) {
        const translatedTitle = await getTranslation(updateNotification.title);
        const newData = {
          MAINTINFO: {
            ...maintenanceInfo,
            title_kr: translatedTitle,
            url: updateNotification.url,
          },
        };
        await saveData(newData);
        return newData;
      }
    }

    // 캐시된 데이터가 유효한지 확인
    const savedData = await loadData();
    if (savedData.MAINTINFO?.end_stamp > now) {
      return savedData;
    }

    // 캐시가 없거나 만료된 경우, 기본 유지보수 공지 사용
    const defaultItem = game.find((item) => item.title?.startsWith("全ワールド"));
    if (!defaultItem) return null;

    const maintenanceInfo = convertToTimestamps(defaultItem);
    if (maintenanceInfo && maintenanceInfo.end_stamp > now) {
      const translatedTitle = await getTranslation(defaultItem.title);
      const newData = {
        MAINTINFO: {
          ...maintenanceInfo,
          title_kr: translatedTitle,
          url: defaultItem.url,
        },
      };
      await saveData(newData);
      return newData;
    }
    return null;
  } catch (error) {
    console.error("Error fetching maintenance data:", error);
    return await getCachedData(now);
  }
}

async function getCachedData(now) {
  // API 호출 실패시 캐시된 데이터 사용
  const savedData = await loadData();
  if (savedData.MAINTINFO?.end_stamp > now) {
    return savedData;
  }
  return null;
}

function convertToTimestamps(item) {
  try {
    // ISO 문자열을 Unix timestamp로 변환
    const startTimestamp = moment(item.start).unix();
    const endTimestamp = moment(item.end).unix();
    
    return {
      start_stamp: startTimestamp,
      end_stamp: endTimestamp
    };
  } catch (error) {
    console.error("Error converting timestamps:", error);
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

function createErrorEmbed() {
  return new EmbedBuilder()
    .setTitle("No Maintenance Info")
    .setDescription("There is no maintenance information available.")
    .setURL("https://jp.finalfantasyxiv.com/lodestone")
    .setColor(EMBED_COLORS.ERROR)
    .setThumbnail(CommandCategory.GAMEINFO?.image);
}