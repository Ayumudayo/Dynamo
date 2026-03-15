const { getSettings } = require("@schemas/Guild");
const { createEtfSession, sendInitialSessionReply } = require("@helpers/StockRefreshSession");
const { normalizeSymbols } = require("@helpers/StockService");

/**
 * Define the ETF command module.
 * @type {import("@structures/Command")}
 */
module.exports = {
  name: "etf", // Command name
  description: "Print ETF data for configured tickers.", // Command description
  cooldown: 10,
  category: "STOCK", // Command category
  botPermissions: ["EmbedLinks"], // Bot permissions required for the bot
  command: {
    enabled: false, // Whether the command is enabled for traditional message usage
    usage: "[command]", // Command usage information
  },
  slashCommand: {
    enabled: true, // Whether the command is enabled for slash command usage
    options: [],
  },

  async interactionRun(interaction, data) {
    const settings = data?.settings || (await getSettings(interaction.guild));
    const tickers = normalizeSymbols(settings.stock_tickers);

    if (!tickers || tickers.length === 0) {
      return interaction.editReply(
        "No stock tickers configured for this server. Please configure them in the dashboard."
      );
    }

    const session = createEtfSession({ tickers });
    const { response } = await sendInitialSessionReply(interaction, session);

    if (!response?.embed) {
      await interaction.editReply("Failed to fetch ETF data. Please try again later.");
    }
  },
};
