const { EMBED_COLORS } = require("@root/config.js");
const {
  EmbedBuilder,
  ActionRowBuilder,
  ButtonBuilder,
  ButtonStyle,
} = require("discord.js");
const fs = require("fs/promises");
const path = require("path");

// 데이터 파일 경로
const dataFilePath = path.join(__dirname, "../../data.json");

/**
 * @type {import("@structures/Command")}
 */
module.exports = {
  name: "wtinv",
  description: "Shows the game invite links (War Thunder / World of Tanks).",
  category: "GAMEINFO",
  botPermissions: ["EmbedLinks"],
  command: { enabled: false, usage: "[command]" },
  slashCommand: { enabled: true, options: [] },

  async messageRun(message) {
    // 텍스트 명령은 미사용 (슬래시만 지원)
    return message.safeReply("이 명령은 슬래시 명령으로 사용하세요: /wtinv");
  },

  async interactionRun(interaction) {
    try {
      const data = await loadData();
      const wtLink = data?.WTINFO?.link; // War Thunder 초대 링크
      const wotLink = data?.WOTINFO?.link; // World of Tanks 초대 링크
      const thumb = data?.WTINFO?.thumbnailLink; // 기본 썸네일 유지

      const embed = new EmbedBuilder()
        .setTitle("Join War Thunder / World of Tanks Now!")
        .setColor(EMBED_COLORS.SUCCESS)
        // .setDescription("아래 버튼을 통해 합류하세요!")
        .setTimestamp()
        .setThumbnail(thumb ?? null);

      const buttons = [];
      if (wtLink) {
        buttons.push(
          new ButtonBuilder()
            .setLabel("War Thunder")
            .setStyle(ButtonStyle.Link)
            .setURL(wtLink)
        );
      }
      if (wotLink) {
        buttons.push(
          new ButtonBuilder()
            .setLabel("World of Tanks")
            .setStyle(ButtonStyle.Link)
            .setURL(wotLink)
        );
      }

      // 버튼이 하나도 없으면 안내만 출력
      if (buttons.length === 0) {
        await interaction.followUp({ embeds: [embed.setDescription("현재 설정된 초대 링크가 없습니다.")] });
        return;
      }

      const row = new ActionRowBuilder().addComponents(...buttons);
      await interaction.followUp({ embeds: [embed], components: [row] });
    } catch (err) {
      console.debug(err);
      await interaction.followUp("처리 중 오류가 발생했습니다.");
    }
  },
};

// JSON 데이터 로드
async function loadData() {
  try {
    const text = await fs.readFile(dataFilePath, "utf-8");
    return JSON.parse(text);
  } catch {
    return {};
  }
}

