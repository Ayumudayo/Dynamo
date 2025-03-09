const { CommandCategory } = require("@src/structures");
const { EMBED_COLORS } = require("@root/config.js");
const { EmbedBuilder, ApplicationCommandOptionType } = require("discord.js");
const { CURRENCIES } = require("@src/data.json");
const ExchangeRateHelper = require("@helpers/exchange.js");

const choices = [
  "USD",
  "KRW",
  "EUR",
  "GBP",
  "JPY",
  "CAD",
  "CHF",
  "HKD",
  "TWD",
  "AUD",
  "NZD",
  "INR",
  "BRL",
  "PLN",
  "RUB",
  "TRY",
  "CNY",
  "UAH",
];

/**
 * @type {import("@structures/Command")}
 */
module.exports = {
  name: "exchange",
  description: "Shows the exchange rate for a given currency.",
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
        description: "The currency you want to convert (From) / Default : USD",
        required: false,
        type: ApplicationCommandOptionType.String,
        choices: choices.map((choice) => ({ name: CURRENCIES[choice], value: choice })),
      },
      {
        name: "to",
        description: "The currency you want to convert (To) / Default : KRW",
        required: false,
        type: ApplicationCommandOptionType.String,
        choices: choices.map((choice) => ({ name: CURRENCIES[choice], value: choice })),
      },
      {
        name: "amount",
        description: "The amount of currency. / Default : 1.0",
        required: false,
        type: ApplicationCommandOptionType.Number,
        minValue: 0,
      },
    ],
  },

  async messageRun(message, args) {
    // ...
  },

  async interactionRun(interaction) {
    try {
      const from = interaction.options.getString("from") || "USD";
      const to = interaction.options.getString("to") || "KRW";
      const amount = interaction.options.getInteger("amount") || 1;

      const res = await getRate(from, to, amount);
      if (!res) {
        await interaction.followUp("Failed to fetch rate data. Please try again later.");
        return;
      }
      await interaction.followUp(res);
    } catch (err) {
      console.debug(err);
    }
  },
};

async function getRate(from, to, amount) {
  //const res = await cc.convert();
  const res = await ExchangeRateHelper.convert(from, to, amount);
  if (!res) {
    console.error("Failed to fetch rate data.");
    return;
  }

  const embed = new EmbedBuilder()
    .setTitle(`Exchange rate from ${from} to ${to}`)
    .setThumbnail(CommandCategory["CURRENCY"].image)
    .setColor(EMBED_COLORS.BOT_EMBED)
    .setFooter({ text: `Data from ExchangeRate-API.` })
    .setTimestamp(Date.now())
    .addFields(
      { name: "From", value: `${amount.toLocaleString({ maximumFractionDigits: 2 })} ${from}` },
      { name: "To", value: `${res.toLocaleString({ maximumFractionDigits: 2 })} ${to}` }
    );

  return { embeds: [embed] };
}
