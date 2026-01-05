const { getSettings } = require("@schemas/Guild");
const { CommandCategory } = require("@src/structures");
const { STOCK, EMBED_COLORS } = require("@root/config.js");
const { EmbedBuilder } = require("discord.js");
const YahooFinance = require('yahoo-finance2').default;
const yahooFinance = new YahooFinance();

/**
 * Define the ETF command module.
 * @type {import("@structures/Command")}
 */
module.exports = {
    name: "etf", // Command name
    description: "Print ETF data for configured tickers.", // Command description
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

    async interactionRun(interaction) {
        const settings = await getSettings(interaction.guild);
        const tickers = settings.stock_tickers;

        if (!tickers || tickers.length === 0) {
            return interaction.followUp("No stock tickers configured for this server. Please configure them in the dashboard.");
        }

        // Send initial response
        let response = await getResultEmbed(tickers);
        if (!response) {
            await interaction.followUp("Failed to fetch ETF data. Please try again later.");
            return;
        }
        await interaction.followUp({ embeds: [response] });

        try {
            // Check if the market is closed before setting up updates
            const stateField = response.data.fields.find(field => field.name === "Market State");
            if (stateField) {
                const state = stateField.value.split(' ')[0];
                if (state === "Closed" || state === "Post" || state === "Unknown") {
                    // If the market is closed or unknown, do not set up the interval for updates
                    return;
                }
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
            response = await getResultEmbed(tickers, updateCount, totalUpdates);

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
 * @param {string[]} tickers - The stock tickers to fetch data for.
 * @param {number} updateCount - The current update count.
 * @param {number} totalUpdates - The total number of updates.
 * @returns {Promise<EmbedBuilder>} - A Promise resolving to an EmbedBuilder with ETF data.
 */
async function getResultEmbed(tickers, updateCount = 0, totalUpdates = STOCK.MAX_REFRESH_TIME / STOCK.REFRESH_INTERVAL) {
    let state = "Unknown";
    let openStatusEmoji = ':black_circle:';
    let isMarketOpen = false;
    let isPreMarket = false;
    let isPostMarket = false;

    try {
        // Fetch temporary stock data to determine market state
        const quoteSummarytmp = await yahooFinance.quoteSummary("NVDA", { modules: ["price"] });
        const resultstmp = quoteSummarytmp.price;
        state = getState(resultstmp);
        isMarketOpen = state === "Regular Market";
        isPreMarket = state === "Pre Market";
        isPostMarket = state === "Post Market";
        openStatusEmoji = isMarketOpen ? ':green_circle:' : (state === isPreMarket) ? ':orange_circle:' : ':red_circle:';
    } catch (err) {
        console.error("Could not fetch market state.", err);
    }

    const embed = new EmbedBuilder()
        .setColor(EMBED_COLORS.BOT_EMBED)
        .setTitle('Configured ETFs')
        .setThumbnail(CommandCategory["STOCK"]?.image)
        .setFooter({ text: `Data from Yahoo Finance. # Update ${updateCount}/${totalUpdates}.` })
        .setTimestamp(Date.now())
        .addFields(
            { name: "Market State", value: `${state} ${openStatusEmoji}`, inline: false },
            { name: ' ', value: ' ', inline: false },
            { name: ' ', value: ' ', inline: false },
        );

    // Create an array of promises for each ETF symbol
    const promises = tickers.map(symbol => yahooFinance.quoteSummary(symbol, { modules: ["price"] }).catch(error => {
        console.error(`Failed to fetch data for ${symbol}: ${error.message}`);
        return { error: error, symbol: symbol }; // Return an object with the error
    }));

    // Use Promise.all to wait for all promises to resolve
    const results = await Promise.all(promises);

    // Process the results
    results.forEach((quoteSummary, index) => {
        if (quoteSummary && !quoteSummary.error) {
            const resultData = quoteSummary.price;
            if (resultData && (isMarketOpen || isPreMarket || isPostMarket)) {
                let priceInfo = (isMarketOpen ? resultData.regularMarketPrice : isPreMarket ? resultData.preMarketPrice : resultData.postMarketPrice);
                let changeInfo = (isMarketOpen ? resultData.regularMarketChange : isPreMarket ? resultData.preMarketChange : resultData.postMarketChange);
                let changePercentInfo = (isMarketOpen ? resultData.regularMarketChangePercent : isPreMarket ? resultData.preMarketChangePercent : resultData.postMarketChangePercent);

                priceInfo = typeof priceInfo === 'number' ? priceInfo : 0;
                changeInfo = typeof changeInfo === 'number' ? changeInfo : 0;
                changePercentInfo = typeof changePercentInfo === 'number' ? changePercentInfo : 0;

                let upDownEmoji = changeInfo > 0 ? '<:yangbonghoro:1162456430360662018>' : changeInfo < 0 ? '<:sale:1162457546532073623>' : '';

                embed.addFields(
                    { name: `${resultData.symbol}`, value: `${resultData.currencySymbol || ''}${priceInfo.toFixed(2)}`, inline: true },
                    { name: "Change", value: `${changeInfo.toFixed(2)} (${(changePercentInfo * 100).toFixed(2)}%) ${upDownEmoji}`, inline: true },
                    { name: ' ', value: ' ', inline: false },
                );
            } else {
                embed.addFields(
                    { name: `${tickers[index]}`, value: `No price data available`, inline: false }
                );
            }
        } else {
            let reason = "Failed to fetch data";
            if (quoteSummary && quoteSummary.error && quoteSummary.error.message.includes("Not Found")) {
                reason = "Invalid Ticker";
            }
            embed.addFields(
                { name: `${tickers[index]}`, value: reason, inline: false }
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
    if (!results || !results.marketState) return "Unknown";
    let state = results['marketState'];
    switch (state) {
        case 'PREPRE': // Fall through
        case 'POST':
        case 'POSTPOST':
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
