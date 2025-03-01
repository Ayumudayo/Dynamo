const { CommandCategory } = require("@src/structures");
const { EMBED_COLORS } = require("@root/config.js");
const { EmbedBuilder } = require("discord.js");
const Parser = require("rss-parser");
const cheerio = require("cheerio");
const fs = require("fs/promises");
const path = require("path");
const moment = require("moment-timezone");
const { translate } = require("@helpers/HttpUtils");

const parser = new Parser();
const dataFilePath = path.join(__dirname, "../../data.json");

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

// PLL 임베드 생성
async function getPLLEmbed() {
  const pllData = await getPLLData();
  if (!pllData) return null;

  const { fixedTitle, start_stamp, url } = pllData;

  return new EmbedBuilder()
    .setTitle(fixedTitle) // 캐시된 fixedTitle 사용
    .setURL(url)
    .setDescription(null)
    .addFields(
      {
        name: "방송 시작",
        value: start_stamp ? `<t:${start_stamp}:F>` : "확인 불가",
        inline: false,
      },
      {
        name: "시작까지 남은 시간",
        value: start_stamp ? `<t:${start_stamp}:R>` : "확인 불가",
        inline: false,
      }
    )
    .setColor(EMBED_COLORS.SUCCESS)
    .setTimestamp()
    .setThumbnail(CommandCategory.GAMEINFO?.image);
}

async function getPLLData() {
  const feedUrl = "https://jp.finalfantasyxiv.com/lodestone/news/topics.xml";
  const now = Date.now() / 1000;

  try {
    // 캐싱 확인
    const savedData = await loadData();
    if (savedData.PLLINFO?.expireTime > now) {
      return savedData.PLLINFO;
    }

    // RSS Feed 파싱
    const feed = await parser.parseURL(feedUrl);
    if (!feed || !feed.items?.length) return null;

    // "第XX回 FFXIV PLL" 형태의 문자열열 찾기
    const targetItem = feed.items.find((item) => {
      return /第\d+回\s?FFXIV\s?PLL/.test(item.title || "");
    });
    if (!targetItem) return null;

    const { title, link, summary } = targetItem;

    // 회차 번호 추출
    const $ = cheerio.load(summary, { decodeEntities: false });
    const h3Element = $("h3.mdl-title__heading--lg").first();
    let roundNumber = "";
    const roundMatch = (h3Element.text() || title).match(/第(\d+)回/);
    if (roundMatch) {
      roundNumber = roundMatch[1];
    }

    // 방송 시작 시각 추출 (전각 괄호 대응)
    let start_stamp = null;
    const dateRegex = /(\d{4}年\d{1,2}月\d{1,2}日（[^）]+）)\s?(\d{1,2}:\d{2})頃?～/;
    const dateMatch = summary.match(dateRegex);
    if (dateMatch) {
      // dateMatch[1] = "2025年3月14日（金）", dateMatch[2] = "19:00"
      const dateStrClean = dateMatch[1].replace(/（[^）]+）/, ""); // → "2025年3月14日"
      const finalStr = `${dateStrClean} ${dateMatch[2]}`; // → "2025年3月14日 19:00"
      const parsed = moment.tz(finalStr, "YYYY年M月D日 HH:mm", "Asia/Tokyo");
      if (parsed.isValid()) {
        start_stamp = parsed.unix();
      }
    }

    let fixedTitle = "제 XX회 프로듀서 레터 라이브 X월 XX일 방송 결정!";
    if (start_stamp) {
      const formattedDate = moment.unix(start_stamp).tz("Asia/Seoul").format("M월 D일");
      const roundText = roundNumber ? `제 ${roundNumber}회` : `제 XX회`;
      fixedTitle = `${roundText} 프로듀서 레터 라이브 ${formattedDate} 방송 결정!`;
    }

    // 캐싱 만료 시간 설정
    const expireTime = Math.floor(now + 12 * 60 * 60);

    const newData = {
      PLLINFO: {
        fixedTitle,
        url: link,
        start_stamp,
        expireTime,
      },
    };

    await saveData(newData);
    return newData.PLLINFO;
  } catch (error) {
    console.error("Error fetching PLL data:", error);
    return null;
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

async function saveData(newData) {
  try {
    const existingData = await loadData();
    await fs.writeFile(dataFilePath, JSON.stringify({ ...existingData, ...newData }, null, 2));
  } catch (error) {
    console.error("Error saving data:", error);
  }
}

// PLL 정보를 찾지 못했을 때 표시할 Embed
function createErrorEmbed() {
  return new EmbedBuilder()
    .setTitle("No PLL Info")
    .setDescription("PLL 관련 정보를 찾을 수 없습니다.")
    .setURL("https://jp.finalfantasyxiv.com/lodestone")
    .setColor(EMBED_COLORS.ERROR)
    .setThumbnail(CommandCategory.GAMEINFO?.image);
}
