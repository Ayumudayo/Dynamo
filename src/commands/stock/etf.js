const { CommandCategory } = require("@src/structures");
const { STOCK, EMBED_COLORS } = require("@root/config.js");
const { EmbedBuilder } = require("discord.js");
const yahooFinance = require('yahoo-finance2').default;

const etfs = [
    'SOXL',
    'SOXS',
    'TQQQ',
    'SQQQ',
    'UPRO',
    'SPY',
    'TLT',
];

/**
 * Define the ETF command module.
 * @type {import("@structures/Command")}
 */
module.exports = {
    name: "etf", // Command name
    description: "Print ETF data list.", // Command description
    category: "STOCK", // Command category
    botPermissions: ["EmbedLinks"], // Bot permissions required for the command
    command: {
        enabled: false, // Whether the command is enabled for traditional message usage
        usage: "[command]", // Command usage information
    },
    slashCommand: {
        enabled: true, // Whether the command is enabled for slash command usage
        options: [],
    },

    async messageRun(message, args) {
        const msg = await message.channel.send("##Fetching data... Please wait##");
        const response = await fetcStockhData(args[1]);
        if (msg.deletable) await msg.delete();
        await message.safeReply(response);
    },

    async interactionRun(interaction) {
        // Send initial response
        let response = await getResultEmbed();
        if (!response) {
            await interaction.followUp("Failed to fetch ETF data. Please try again later.");
            return;
        }
        await interaction.followUp({ embeds: [response] });

        try {
            // Check if the market is closed before setting up updates
            const state = response.data.fields.find(field => field.name === "Market State").value.split(' ')[0];
            if (state === "Closed" || state === "Post") {
                // If the market is closed, do not set up the interval for updates
                console.debug("CLOSED");
                return;
            }
        }
        catch (err) {
            console.debug(err);
        }

        let updateCount = 0;
        const totalUpdates = STOCK.MAX_REFRESH_TIME / STOCK.REFRESH_INTERVAL;

        // Update the response every REFRESH_INTERVAL milliseconds
        const interval = setInterval(async () => {
            updateCount++; // Increment the update count
            // Fetch new data
            response = await getResultEmbed(updateCount, totalUpdates);
            
            if (response) {
                // Edit the original reply with the new data
                await interaction.editReply({ embeds: [response] }).catch(console.error);
            }
            // If we've reached the total number of updates, clear the interval
            if (updateCount >= totalUpdates) {
                clearInterval(interval);
            }
        }, STOCK.REFRESH_INTERVAL);
    }
};

/**
 * Helper function to fetch ETF data and create an embed.
 * @param {number} updateCount - The current update count.
 * @param {number} totalUpdates - The total number of updates.
 * @returns {Promise<EmbedBuilder>} - A Promise resolving to an EmbedBuilder with ETF data.
 */
async function getResultEmbed(updateCount = 0, totalUpdates = STOCK.MAX_REFRESH_TIME / STOCK.REFRESH_INTERVAL) {
    // Fetch temporary stock data to determine market state
    const quoteSummarytmp = await yahooFinance.quoteSummary("NVDA", { modules: ["price"] });
    const resultstmp = quoteSummarytmp.price;

    const state = getState(resultstmp);
    const isMarketOpen = state === "Regular Market";
    const isPreMarket = state === "Pre Market";
    const isPostMarket = state === "Post Market";

    const openStatusEmoji = isMarketOpen ? ':green_circle:' : (state === isPreMarket) ? ':orange_circle:' : ':red_circle:';

    const embed = new EmbedBuilder()
        .setColor(EMBED_COLORS.BOT_EMBED)
        .setTitle('ETF Lists')
        .setThumbnail(CommandCategory["STOCK"]?.image)
        .setFooter({ text: `Data from Yahoo Finance. # Update ${updateCount}/${totalUpdates}.` })
        .setTimestamp(Date.now())
        .addFields(
            { name: "Market State", value: `${state} ${openStatusEmoji}`, inline: false },
            { name: ' ', value: ' ', inline: false },
            { name: ' ', value: ' ', inline: false },
        );

    // Create an array of promises for each ETF symbol
    const promises = etfs.map(symbol => yahooFinance.quoteSummary(symbol, { modules: ["price"] }).catch(error => {
        console.error(`Failed to fetch data for ${symbol}: ${error}`);
        return null; // Return null if there's an error
    }));

    // Use Promise.all to wait for all promises to resolve
    const results = await Promise.all(promises);

    // Process the results
    results.forEach((quoteSummary, index) => {
        if (quoteSummary) {
            const results = quoteSummary.price;
            // Only add data to the embed if the market state is regular, pre, or post
            if (isMarketOpen || isPreMarket || isPostMarket) {
                let priceInfo = isMarketOpen ? results.regularMarketPrice : isPreMarket ? results.preMarketPrice : results.postMarketPrice;
                let changeInfo = isMarketOpen ? results.regularMarketChange : isPreMarket ? results.preMarketChange : results.postMarketChange;
                let changePercentInfo = isMarketOpen ? results.regularMarketChangePercent : isPreMarket ? results.preMarketChangePercent : results.postMarketChangePercent;
                let upDownEmoji = changeInfo > 0 ? '<:yangbonghoro:1162456430360662018>' : changeInfo < 0 ? '<:sale:1162457546532073623>' : '';

                embed.addFields(
                    { name: `${results.symbol}`, value: `${results.currencySymbol}${priceInfo.toFixed(2)}`, inline: true },
                    { name: "Change", value: `${changeInfo.toFixed(2)} (${(changePercentInfo * 100).toFixed(2)}%) ${upDownEmoji}`, inline: true },
                    { name: ' ', value: ' ', inline: false },
                );
            }
        } else {
            // If the result is null, it means there was an error fetching the data for this symbol
            embed.addFields(
                { name: `${etfs[index]}`, value: `Failed to fetch data`, inline: false }
            );
        }
    });

    return embed;
}

/**
 * Helper function to determine the market state based on Yahoo Finance results.
 * @param {object} results - Yahoo Finance results object.
 * @returns {string} - Market state string.
 */
function getState(results) {
    let state = results['marketState'];
    switch (state) {
        case 'PREPRE': // Fall through
        case 'POST':
        case 'CLOSED':
            return "Post Market";
        case 'PRE':
            return "Pre Market";
        case 'REGULAR':
            return "Regular Market";
        default:
            return "Unknown";
    }
}
