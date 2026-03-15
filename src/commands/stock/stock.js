const { ApplicationCommandOptionType } = require("discord.js");
const { createStockSession, sendInitialSessionReply } = require("@helpers/StockRefreshSession");
const { DEFAULT_SYMBOL, getSingleStockResponse, getTotalUpdates, normalizeSymbol } = require("@helpers/StockService");

/**
 * Define the stock command module.
 * @type {import("@structures/Command")}
 */
module.exports = {
  name: "stock", // Command name
  description: "Print stock data for the given symbol.", // Command description
  cooldown: 10,
  category: "STOCK", // Command category
  botPermissions: ["EmbedLinks"], // Bot permissions required for the bot
  command: {
    enabled: false, // Whether the command is enabled for traditional message usage
    usage: "[command] [symbol]", // Command usage information
  },
  slashCommand: {
    enabled: true, // Whether the command is enabled for slash command usage
    options: [
      {
        name: "symbol",
        description: "Symbol of the stock",
        required: false,
        type: ApplicationCommandOptionType.String,
      },
    ],
  },

  // Handler function for traditional message usage
  async messageRun(message, args) {
    const symbol = normalizeSymbol(args[0] || DEFAULT_SYMBOL);
    const totalUpdates = getTotalUpdates();
    const response = await getSingleStockResponse(symbol, { updateCount: 0, totalUpdates });

    if (!response?.embed) return message.channel.send("Failed to fetch stock data. Please try again later.");
    return message.channel.send({ embeds: [response.embed] });
  },

  // Handler function for slash command usage
  async interactionRun(interaction) {
    const symbol = normalizeSymbol(interaction.options.getString("symbol") || DEFAULT_SYMBOL);
    const session = createStockSession({ symbol });
    const { response } = await sendInitialSessionReply(interaction, session);

    if (!response?.embed) {
      await interaction.editReply("Failed to fetch stock data. Please try again later.");
    }
  },
};
