const { CommandCategory } = require("@src/structures");
const { EMBED_COLORS } = require("@root/config.js");
const { EmbedBuilder } = require("discord.js");
const Parser = require("rss-parser");
const cheerio = require("cheerio");
const fs = require("fs/promises");
const path = require("path");
const moment = require("moment-timezone");

const parser = new Parser();
const dataFilePath = path.join(__dirname, "../../data.json");
const CACHE_DURATION = 12 * 60 * 60; // 12시간 (초)
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
      console.error(err);
      await interaction.followUp("An error occurred while processing your request.");
    }
  },
};

async function getPLLEmbed() {
  const pllData = await getPLLData();
  if (!pllData) return null;

  const { fixedTitle, start_stamp, url } = pllData;
  const timeValue = start_stamp ? `<t:${start_stamp}:F>` : "확인 불가";
  const relativeTimeValue = start_stamp ? `<t:${start_stamp}:R>` : "확인 불가";

  return new EmbedBuilder()
    .setTitle(fixedTitle)
    .setURL(url)
    .addFields(
      { name: "방송 시작", value: timeValue, inline: false },
      { name: "시작까지 남은 시간", value: relativeTimeValue, inline: false }
    )
    .setColor(EMBED_COLORS.SUCCESS)
    .setTimestamp()
    .setThumbnail(CommandCategory.GAMEINFO?.image)
    .setFooter({ text: "From Lodestone News" });
}

async function getPLLData() {
  const now = Date.now() / 1000;

  // 캐시 확인
  const cachedData = await getCachedData(now);
  if (cachedData) return cachedData;

  try {
    const feed = await parser.parseURL("https://jp.finalfantasyxiv.com/lodestone/news/topics.xml");
    if (!feed?.items?.length) return null;

    const targetItem = feed.items.find(item => PLL_TITLE_REGEX.test(item.title || ""));
    if (!targetItem) return null;

    const pllInfo = await processPLLItem(targetItem);
    if (!pllInfo) return null;

    const newData = {
      PLLINFO: {
        ...pllInfo,
        expireTime: Math.floor(now + CACHE_DURATION),
      },
    };

    await saveData(newData);
    return newData.PLLINFO;
  } catch (error) {
    console.error("Error fetching PLL data:", error);
    return null;
  }
}

async function processPLLItem(item) {
  const { title, link, summary } = item;
  
  // cheerio를 사용해 summary에서 정보 추출
  const $ = cheerio.load(summary, { decodeEntities: false });
  const h3Text = $("h3.mdl-title__heading--lg").first().text() || title;
  
  // 회차 번호 추출
  const roundMatch = h3Text.match(ROUND_REGEX);
  const roundNumber = roundMatch?.[1] || "";
  
  // 방송 시작 시각 추출
  const start_stamp = extractStartTime(summary);
  
  // 제목 생성
  const fixedTitle = generateFixedTitle(roundNumber, start_stamp);
  
  return {
    fixedTitle,
    url: link,
    start_stamp,
  };
}

function extractStartTime(summary) {
  const dateMatch = summary.match(DATE_REGEX);
  if (!dateMatch) return null;

  const dateStrClean = dateMatch[1].replace(/（[^）]+）/, "");
  const timeString = `${dateStrClean} ${dateMatch[2]}`;
  const parsed = moment.tz(timeString, "YYYY年M月D日 HH:mm", "Asia/Tokyo");
  
  return parsed.isValid() ? parsed.unix() : null;
}

function generateFixedTitle(roundNumber, start_stamp) {
  if (!start_stamp) {
    return "제 XX회 프로듀서 레터 라이브 X월 XX일 방송 결정!";
  }

  const formattedDate = moment.unix(start_stamp).tz("Asia/Seoul").format("M월 D일");
  const roundText = roundNumber ? `제 ${roundNumber}회` : "제 XX회";
  
  return `${roundText} 프로듀서 레터 라이브 ${formattedDate} 방송 결정!`;
}

async function getCachedData(now) {
  const savedData = await loadData();
  return savedData.PLLINFO?.expireTime > now ? savedData.PLLINFO : null;
}

async function loadData() {
  try {
    const data = await fs.readFile(dataFilePath, "utf-8");
    return JSON.parse(data);
  } catch (error) {
    return {};
  }
}

async function saveData(newData) {
  try {
    const existingData = await loadData();
    const mergedData = { ...existingData, ...newData };
    await fs.writeFile(dataFilePath, JSON.stringify(mergedData, null, 2));
  } catch (error) {
    console.error("Error saving data:", error);
  }
}

function createErrorEmbed() {
  return new EmbedBuilder()
    .setTitle("No PLL Info")
    .setDescription("PLL 관련 정보를 찾을 수 없습니다.")
    .setURL("https://jp.finalfantasyxiv.com/lodestone")
    .setColor(EMBED_COLORS.ERROR)
    .setThumbnail(CommandCategory.GAMEINFO?.image);
}