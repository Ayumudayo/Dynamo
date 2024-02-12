const { CommandCategory } = require("@src/structures");
const { EMBED_COLORS } = require("@root/config.js");
const { EmbedBuilder } = require("discord.js");
const Parser = require('rss-parser');
const fs = require('fs/promises');
const path = require('path');
const moment = require("moment");
const { translate } = require("@helpers/HttpUtils");

const parser = new Parser();
const dataFilePath = path.join(__dirname, '../../data.json');

module.exports = {
    name: "maint",
    description: "Shows the FFXIV maintenance information",
    category: "GAMEINFO",
    botPermissions: ["EmbedLinks"],
    command: { enabled: false, usage: "[command]" },
    slashCommand: { enabled: true, options: [] },
    
    async interactionRun(interaction) {
        try {
            const embed = await getMaintenanceEmbed();
            await interaction.followUp({ embeds: [embed ? embed : createErrorEmbed()] });
        } catch (err) {
            console.error(err);
            await interaction.followUp("An error occurred while processing your request.");
        }
    }
};

/**
 * Fetches maintenance data and constructs an embed for display.
 * @async
 * @returns {EmbedBuilder|null} The constructed embed with maintenance information or null if no data is available.
 */
async function getMaintenanceEmbed() {
    const maintData = await getMaintData();
    if (!maintData) return null;

    const { start_stamp, end_stamp, title_kr, url } = maintData.MAINTINFO;
    return new EmbedBuilder()
        .setTitle(title_kr)
        .setURL(url)
        .addFields(
            { name: "Begin", value: `<t:${start_stamp}:F>`, inline: false },
            { name: "End", value: `<t:${end_stamp}:F>`, inline: false },
            { name: "Until End", value: `<t:${end_stamp}:R>`, inline: false },
        )
        .setColor(EMBED_COLORS.SUCCESS)
        .setTimestamp()
        .setThumbnail(CommandCategory.GAMEINFO?.image);
}

/**
 * Fetches and processes maintenance data from the configured RSS feed.
 * @async
 * @returns {Object|null} Processed maintenance data or null if not available.
 */
async function getMaintData() {
    const feedUrl = 'https://jp.finalfantasyxiv.com/lodestone/news/news.xml';
    try {
        const savedData = await loadData();
        const feed = await parser.parseURL(feedUrl);
        const targetItem = feed.items.find(item => item.title?.startsWith('全ワールド'));

        if (!targetItem) return null;
        const maintenanceInfo = extractMaintenanceInfo(targetItem);

        if (maintenanceInfo && maintenanceInfo.end_stamp > Date.now() / 1000) {
            const translatedTitle = await getTranslation(targetItem.title);
            const newData = { MAINTINFO: { ...maintenanceInfo, title_kr: translatedTitle, url: targetItem.link } };
            await saveData(newData);
            return newData;
        }

        return savedData.MAINTINFO && savedData.MAINTINFO.end_stamp > Date.now() / 1000 ? savedData : null;
    } catch (error) {
        console.error('Error fetching maintenance data:', error);
        return null;
    }
}

/**
 * Translates the given text to Korean using a translation service.
 * @async
 * @param {string} input - The text to be translated.
 * @returns {Promise<string>} The translated text.
 */
async function getTranslation(input) {
    const data = await translate(input, "ko");
    if (!data) return "Failed to translate the title";

    return data.output;
}

/**
 * Saves the provided data to a JSON file.
 * @async
 * @param {Object} newData - The data to be saved.
 */
async function saveData(newData) {
    try {
        const existingData = await loadData();
        await fs.writeFile(dataFilePath, JSON.stringify({ ...existingData, ...newData }, null, 2));
    } catch (error) {
        console.error('Error saving data:', error);
    }
}

/**
 * Loads and returns data from a JSON file.
 * @async
 * @returns {Promise<Object>} The loaded data.
 */
async function loadData() {
    try {
        const data = await fs.readFile(dataFilePath, 'utf-8');
        return JSON.parse(data);
    } catch (error) {
        return {};
    }
}

/**
 * Extracts maintenance information from an RSS feed item.
 * @param {Object} item - The feed item containing maintenance information.
 * @returns {Object|null} Extracted maintenance information or null if not found.
 */
function extractMaintenanceInfo(item) {
    const startTimeRegex = /日　時：(\d{4}年\d{1,2}月\d{1,2}日\(.\)) (\d{1,2}:\d{2})より/;
    const endTimeRegex = /(\d{4}年\d{1,2}月\d{1,2}日\(.\))? ?(\d{1,2}:\d{2})頃まで/;
    const startTimeMatch = item.content.match(startTimeRegex);
    const endTimeMatch = item.content.match(endTimeRegex);
    if (!startTimeMatch || !endTimeMatch) return null;

    const startTime = moment(`${startTimeMatch[1]} ${startTimeMatch[2]}`, "YYYY年MM月DD日(ddd) HH:mm", "ja").unix();
    const endTime = endTimeMatch[1] ? moment(`${endTimeMatch[1]} ${endTimeMatch[2]}`, "YYYY年MM月DD日(ddd) HH:mm", "ja").unix() : moment(`${startTimeMatch[1]} ${endTimeMatch[2]}`, "YYYY年MM月DD日(ddd) HH:mm", "ja").unix();

    return { start_stamp: startTime, end_stamp: endTime };
}

/**
 * Creates an error embed to be displayed when maintenance information is unavailable.
 * @returns {EmbedBuilder} An embed indicating the absence of maintenance information.
 */
function createErrorEmbed() {
    return new EmbedBuilder()
        .setTitle("No Maintenance Info")
        .setDescription("There is no maintenance information available.")
        .setURL("https://jp.finalfantasyxiv.com/lodestone")
        .setColor(EMBED_COLORS.ERROR)
        .setThumbnail(CommandCategory.GAMEINFO?.image);
}