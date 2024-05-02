const { CommandCategory } = require("@src/structures");
const { EMBED_COLORS } = require("@root/config.js");
const { EmbedBuilder, ApplicationCommandOptionType } = require("discord.js");
const CurrencyConverter = require('currency-converter-lt');
const { CURRENCIES } = require("@src/data.json");

// Define an array of currency codes and their corresponding flag emojis
const currencyEmojis = {
    "USD": "🇺🇸",
    "KRW": "🇰🇷",
    "JPY": "🇯🇵",
    "EUR": "🇪🇺",
    "TRY": "🇹🇷",
    "UAH": "🇺🇦"
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
    }
};

// Function to get exchange rates and create an embed with the results
async function getRate(from, amount) {
    const cc = new CurrencyConverter();
    cc.amount(amount);

    const embed = new EmbedBuilder()
        .setTitle(`Exchange rate from ${amount} ${from}`)
        .setThumbnail(CommandCategory["CURRENCY"].image)
        .setColor(EMBED_COLORS.BOT_EMBED)
        .setFooter({ text: `Data from Google.` })
        .setTimestamp(Date.now());

    // Get the list of target currencies from the CURRENCIES object keys
    const targetCur = Object.keys(currencyEmojis);

    // Create a promise for each currency conversion
    const promises = targetCur.map(async (cur) => {
        try {
            const rated = await cc.from(from).to(cur).convert();
            return { currency: cur, rate: rated };
        } catch (error) {
            console.error(`Failed to fetch data for ${cur}. ${error}`);
            return { currency: cur, rate: null };
        }
    });

    // Wait for all promises to settle (either resolve or reject)
    const results = await Promise.allSettled(promises);

    // Process the results and add them to the embed
    results.forEach((result) => {
        if (result.status === "fulfilled" && result.value.rate !== null) {
            const { currency, rate } = result.value;
            const emoji = currencyEmojis[currency];
            embed.addFields({ name: `${emoji} ${currency}`, value: rate.toLocaleString(undefined, { maximumFractionDigits: 2 }), inline: true });
        } else if (result.status === "fulfilled") {
            const { currency } = result.value;
            const emoji = currencyEmojis[currency];
            embed.addFields({ name: `${emoji} ${currency}`, value: "Failed to fetch", inline: true });
        }
    });

    return { embeds: [embed] };
}
