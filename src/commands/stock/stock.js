const { CommandCategory } = require("@src/structures");
const { STOCK } = require("@root/config.js");
const { EmbedBuilder, ApplicationCommandOptionType } = require("discord.js");
const yahooFinance = require('yahoo-finance2').default;

/**
 * Define the stock command module.
 * @type {import("@structures/Command")}
 */
module.exports = {
  name: "stock", // Command name
  description: "Print stock data for the given symbol.", // Command description
  category: "STOCK", // Command category
  botPermissions: ["EmbedLinks"], // Bot permissions required for the command
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
    // Retrieve the stock symbol from the command arguments or default to 'NVDA'
    const symbol = args[0] || 'NVDA';
    
    // Fetch stock data and send the result as an embed
    const response = await getResultEmbed(symbol);
    if (response) {
      await message.channel.send(response);
    } else {
      await message.channel.send("Failed to fetch stock data. Please try again later.");
    }
  },

  // Handler function for slash command usage
  async interactionRun(interaction) {
    // Retrieve the stock symbol from the slash command options or default to 'NVDA'
    let symbol = interaction.options.getString("symbol") || 'NVDA';

    // Send initial response with stock data
    let response = await getResultEmbed(symbol);
    if (!response) {
      await interaction.followUp("Failed to fetch stock data. Please try again later.");
      return;
    }
    await interaction.followUp({ embeds: [response] });

    // Check if the market is closed before setting up updates
    const state = response.data.fields.find(field => field.name === "Market State").value.split(' ')[0];
    if (state === "Closed" || state === "Post") {
      // If the market is closed, do not set up the interval for updates
      return;
    }

    // Set up periodic updates for the stock data
    let updateCount = 0;
    const totalUpdates = STOCK.MAX_REFRESH_TIME / STOCK.REFRESH_INTERVAL;
    
    // Update the response every REFRESH_INTERVAL milliseconds
    const interval = setInterval(async () => {
      updateCount++; // Increment the update count
      
      // Fetch new data
      response = await getResultEmbed(symbol, updateCount, totalUpdates);
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
 * Helper function to fetch stock data and create an embed.
 * @param {string} symbol - The stock symbol to fetch data for.
 * @param {number} updateCount - The current update count.
 * @param {number} totalUpdates - The total number of updates.
 * @returns {Promise<EmbedBuilder|null>} - A Promise resolving to an EmbedBuilder or null if fetching fails.
 */
async function getResultEmbed(symbol, updateCount = 0, totalUpdates) {
  try {
    // Fetch stock data using the Yahoo Finance API
    const quoteSummary = await yahooFinance.quoteSummary(symbol, { modules: ["price"] });
    const results = quoteSummary.price;

    // Determine the market state and emojis for open/closed status
    const state = getState(results);
    const isMarketOpen = state === "Regular Market";
    const isPreMarket = state === "Pre Market";
    const isPostMarket = state === "Post Market";
    const openStatusEmoji = isMarketOpen ? ':green_circle:' : (state === isPreMarket) ? ':orange_circle:' : ':red_circle:';

    // Create an embed with stock data
    const embed = new EmbedBuilder()
      .setTitle(`${results.longName} / [${results.symbol}]`)
      .setURL(`https://finance.yahoo.com/quote/${results.symbol}`)
      .setThumbnail(CommandCategory.STOCK?.image)
      .addFields(
        { name: "Market State", value: `${state} ${openStatusEmoji}`, inline: false },
        { name: ' ', value: ' ', inline: false },
        { name: ' ', value: ' ', inline: false },
      );

      let upDownEmoji = results.regularMarketChange > 0 ? '<:yangbonghoro:1162456430360662018>' : results.regularMarketChange < 0 ? '<:sale:1162457546532073623>' : '';
      embed.addFields(
        { name: "Price", value: `${results.currencySymbol}${results.regularMarketPrice.toFixed(2)}`, inline: true },
        { name: "Change", value: `${results.regularMarketChange.toFixed(2)} (${(results.regularMarketChangePercent * 100).toFixed(2)}%) ${upDownEmoji}`, inline: true },
        { name: ' ', value: ' ', inline: false },
      )      
      .setColor(getEmbedColor(results))
      .setFooter({ text: `Data from Yahoo Finance. #Update ${updateCount}/${totalUpdates || '∞'}.` })
      .setTimestamp(Date.now());

    // Add preMarket and postMarket fields if applicable
    if (isPreMarket) {
      upDownEmoji = results.preMarketChange > 0 ? '<:yangbonghoro:1162456430360662018>' : results.preMarketChange < 0 ? '<:sale:1162457546532073623>' : '';
      embed.addFields(
        { name: "Pre - Price", value: `${results.currencySymbol}${results.preMarketPrice.toFixed(2)}`, inline: true },
        { name: "Pre - Change", value: `${results.preMarketChange.toFixed(2)} (${(results.preMarketChangePercent * 100).toFixed(2)}%) ${upDownEmoji}`, inline: true },
        { name: ' ', value: ' ', inline: false },
      );
    } else if (isPostMarket) {
      upDownEmoji = results.postMarketChange > 0 ? '<:yangbonghoro:1162456430360662018>' : results.postMarketChange < 0 ? '<:sale:1162457546532073623>' : '';
      embed.addFields(
        { name: "Post - Price", value: `${results.currencySymbol}${results.postMarketPrice.toFixed(2)}`, inline: true },
        { name: "Post - Change", value: `${results.postMarketChange.toFixed(2)} (${(results.postMarketChangePercent * 100).toFixed(2)}%) ${upDownEmoji}`, inline: true },
        { name: ' ', value: ' ', inline: false },
      );
    }

    // Add additional fields to the embed
    embed.addFields(
      { name: "Day High", value: `${results.currencySymbol}${results.regularMarketDayHigh.toFixed(2)}`, inline: true },
      { name: "Day Low", value: `${results.currencySymbol}${results.regularMarketDayLow.toFixed(2)}`, inline: true },
      { name: "Volume", value: results.regularMarketVolume.toLocaleString(), inline: true },
    )

    // Return the created embed
    return embed;
  } catch (error) {
    // Log an error if fetching stock data fails
    console.error(`Failed to fetch stock data for symbol: ${symbol}`, error);
    return null;
  }
}

/**
 * Helper function to determine the market state based on Yahoo Finance results.
 * @param {object} results - Yahoo Finance results object.
 * @returns {string} - Market state string.
 */
function getState(results) {
  switch (results.marketState) {
    case 'PREPRE':
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

/**
 * Helper function to determine the embed color based on stock change.
 * @param {object} results - Yahoo Finance results object.
 * @returns {string} - Embed color.
 */
function getEmbedColor(results) {
  if (results.regularMarketChange > 0) {
    return STOCK.UPWARD_EMBED;
  } else if (results.regularMarketChange < 0) {
    return STOCK.DOWNWARD_EMBED;
  } else {
    return STOCK.DEFAULT_EMBED;
  }
}