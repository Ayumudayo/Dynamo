const { CommandCategory } = require("@src/structures");
const { EMBED_COLORS } = require("@root/config.js");
const { EmbedBuilder, ApplicationCommandOptionType } = require("discord.js");
const { CURRENCIES } = require("@src/data.json");
const ExchangeRateHelper = require("@helpers/exchange.js");

const DEFAULT_TARGET_CURRENCIES = ["USD", "KRW", "JPY", "EUR", "TRY", "UAH"];

// Define an array of currency codes and their corresponding flag emojis
const currencyEmojis = {
  USD: "🇺🇸",
  KRW: "🇰🇷",
  JPY: "🇯🇵",
  EUR: "🇪🇺",
  TRY: "🇹🇷",
  UAH: "🇺🇦",
};

/**
 * @type {import("@structures/Command")}
 */
module.exports = {
  name: "rate",
  description: "Shows the exchange rate list.",
  category: "CURRENCY",
  botPermissions: ["EmbedLinks"],
  command: {
    enabled: false,
    usage: "[command]",
  },
  slashCommand: {
    enabled: true,
    options: [
      {
        name: "from",
        description: "The currency you want to convert from (default: USD)",
        required: false,
        type: ApplicationCommandOptionType.String,
        // Generate choices from the CURRENCIES object keys
        choices: Object.keys(CURRENCIES).map((key) => ({ name: CURRENCIES[key], value: key })),
      },
      {
        name: "amount",
        description: "The amount of currency (default: 1.0)",
        required: false,
        type: ApplicationCommandOptionType.Number,
        minValue: 0,
      },
    ],
  },

  // Handler for slash command interactions
  async interactionRun(interaction) {
    try {
      const from = interaction.options.getString("from") || "USD";
      const amount = interaction.options.getNumber("amount") || 1;

      const res = await getRate(from, amount);
      if (!res) {
        await interaction.followUp("Failed to fetch rate data. Please try again later.");
        return;
      }
      await interaction.followUp(res);
    } catch (err) {
      console.error(err);
      await interaction.followUp("An error occurred while processing your request.");
    }
  },
};

async function getRate(from, amount) {
  const embed = new EmbedBuilder()
    .setTitle(`Exchange rate from ${amount} ${from}`)
    .setThumbnail(CommandCategory["CURRENCY"].image)
    .setColor(EMBED_COLORS.BOT_EMBED)
    .setFooter({ text: `Data from ExchangeRate-API.` })
    .setTimestamp(Date.now());

  const conversionResults = await ExchangeRateHelper.getRates(from, amount, DEFAULT_TARGET_CURRENCIES);

  conversionResults.forEach(({ currency, rate }) => {
    const emoji = currencyEmojis[currency] || "";
    if (rate !== null && rate !== undefined) {
      embed.addFields({
        name: `${emoji} ${currency}`,
        value: rate.toLocaleString(undefined, { maximumFractionDigits: 2 }),
        inline: true,
      });
    } else {
      embed.addFields({
        name: `${emoji} ${currency}`,
        value: "Failed to fetch",
        inline: true,
      });
    }
  });

  return { embeds: [embed] };
}
