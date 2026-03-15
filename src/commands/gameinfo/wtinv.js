const { EMBED_COLORS } = require("@root/config.js");
const Logger = require("@helpers/Logger");
const { EmbedBuilder, ActionRowBuilder, ButtonBuilder, ButtonStyle } = require("discord.js");

const gameInfoDataStore = require("@helpers/GameInfoDataStore");

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
    // Text commands are not supported (slash-only).
    return message.safeReply("Please use /wtinv instead.");
  },

  async interactionRun(interaction) {
    try {
      const data = await loadData();
      const wtLink = toValidUrl(data?.WTINFO?.link);
      const wotLink = toValidUrl(data?.WOTINFO?.link);
      const thumb = toValidUrl(data?.WTINFO?.thumbnailLink);

      const embed = new EmbedBuilder()
        .setTitle("Join War Thunder / World of Tanks Now!")
        .setColor(EMBED_COLORS.SUCCESS)
        .setTimestamp()
        .setThumbnail(thumb ?? null);

      const buttons = [];
      if (wtLink) {
        buttons.push(new ButtonBuilder().setLabel("War Thunder").setStyle(ButtonStyle.Link).setURL(wtLink));
      }
      if (wotLink) {
        buttons.push(new ButtonBuilder().setLabel("World of Tanks").setStyle(ButtonStyle.Link).setURL(wotLink));
      }

      // If no button links are set, only show message.
      if (buttons.length === 0) {
        await interaction.followUp({
          embeds: [embed.setDescription("No invite links are currently configured.")],
        });
        return;
      }

      const row = new ActionRowBuilder().addComponents(...buttons);
      await interaction.followUp({ embeds: [embed], components: [row] });
    } catch (err) {
      Logger.error("wtinv interactionRun", err);
      await interaction.followUp("An error occurred while processing your request.");
    }
  },
};

async function loadData() {
  return gameInfoDataStore.load();
}

function toValidUrl(value) {
  if (typeof value !== "string" || !value.trim()) return null;

  try {
    return new URL(value).toString();
  } catch {
    return null;
  }
}
