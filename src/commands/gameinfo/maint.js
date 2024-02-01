const { CommandCategory } = require("@src/structures");
const { EMBED_COLORS } = require("@root/config.js");
const { EmbedBuilder } = require("discord.js");
const Parser = require('rss-parser');
const fs = require('fs/promises');
const path = require('path');
const moment = require("moment");
const { translate } = require("@helpers/HttpUtils");

// Initialize an RSS parser
const parser = new Parser();

// Set the path to the data file
const dataFilePath = path.join(__dirname, '../../data.json');

/**
 * @type {import("@structures/Command")}
 */
module.exports = {
    name: "maint",
    description: "Shows the FFXIV maintenance information",
    category: "GAMEINFO",
    botPermissions: ["EmbedLinks"],
    command: {
        enabled: false,
        usage: "[command]",
    },
    slashCommand: {
        enabled: true,
        options: [],
    },

    async messageRun(message, args) {
        // NOT IMPLEMENTED
    },

    async interactionRun(interaction) {
        try {
            const res = await getResultEmbed();
            if (!res) {
                await interaction.followUp("Failed to fetch data. Please try again later.");
                return;
            }
            await interaction.followUp({ embeds: [res] });
        } catch (err) {
            console.error(err);
            await interaction.followUp("An error occurred while processing your request.");
        }
    }
};

/**
 * Saves the provided data to the data file.
 * @param {object} newData - The new data to be saved.
 */
async function saveData(newData) {
    try {
        const existingData = await loadData();
        existingData.MAINTINFO = newData.MAINTINFO;
        const jsonData = JSON.stringify(existingData, null, 2);
        await fs.writeFile(dataFilePath, jsonData);
    } catch (error) {
        console.error('Error saving data:', error);
    }
}

/**
 * Loads data from the data file.
 * @returns {object} The loaded data.
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
 * Retrieves maintenance data from the Lodestone RSS feed.
 * @returns {object|null} The maintenance data or null if no relevant data is found.
 */
async function getMaintData() {
    try {
        const feedUrl = 'https://jp.finalfantasyxiv.com/lodestone/news/news.xml';
        const savedData = await loadData();

        const feed = await parser.parseURL(feedUrl);
        const targetItem = feed.items.find(item => item.title && item.title.startsWith('全ワールド'));

        if (!targetItem) return null;

        const content = targetItem.content;
        const startTimeRegex = /日　時：(\d{4}年\d{1,2}月\d{1,2}日\(.\)) (\d{1,2}:\d{2})より/;
        const endTimeRegex = /(\d{4}年\d{1,2}月\d{1,2}日\(.\)) (\d{1,2}:\d{2})頃まで/;

        const startTimeMatch = content.match(startTimeRegex);
        const endTimeMatch = content.match(endTimeRegex);

        if (!startTimeMatch || !endTimeMatch) return null;

        const startTime = moment(`${startTimeMatch[1]} ${startTimeMatch[2]}`, "YYYY年MM月DD日(ddd) HH:mm", "ja").unix();
        const endTime = moment(`${endTimeMatch[1]} ${endTimeMatch[2]}`, "YYYY年MM月DD日(ddd) HH:mm", "ja").unix();

        const currentTimestamp = Date.now();

        if (savedData.MAINTINFO && savedData.MAINTINFO.end_stamp > currentTimestamp) {
            return savedData;
        } else if (endTime > currentTimestamp) {
            const translatedTitle = await getTranslation(targetItem.title);
            const newData = {
                MAINTINFO: {
                    start_stamp: startTime,
                    end_stamp: endTime,
                    title: targetItem.title,
                    title_kr: translatedTitle,
                    url: targetItem.link
                }
            };

            await saveData(newData);
            return newData;
        } else {
            return null;
        }
    } catch (error) {
        console.error('Error:', error);
        return null;
    }
}

/**
 * Retrieves the translated title using a translation service.
 * @param {string} input - The input text to be translated.
 * @returns {string} The translated title or a failure message.
 */
async function getTranslation(input) {
    const data = await translate(input, "ko");
    if (!data) return "Failed to translate the title";

    return data.output;
}

/**
 * Creates and returns an embed for the maintenance information.
 * @returns {object} The embed containing maintenance information.
 */
async function getResultEmbed() {
    try {
        const maintData = await getMaintData();
        if (!maintData) {
            return createErrorEmbed();
        }

        return new EmbedBuilder()
            .setTitle(maintData.MAINTINFO.title_kr)
            .setURL(maintData.MAINTINFO.url)
            .addFields(
                { name: "Start", value: `<t:${maintData.MAINTINFO.start_stamp}:F>`, inline: false },
                { name: "End", value: `<t:${maintData.MAINTINFO.end_stamp}:F>`, inline: false },
                { name: "Until End", value: `<t:${maintData.MAINTINFO.end_stamp}:R>`, inline: false },
            )
            .setColor(EMBED_COLORS.SUCCESS)
            .setTimestamp(Date.now())
            .setThumbnail(CommandCategory.GAMEINFO?.image);
    } catch (err) {
        console.error('Error:', err);
        return createErrorEmbed();
    }
}

/**
 * Creates and returns an embed for the case where no maintenance information is available.
 * @returns {object} The embed indicating no maintenance information.
 */
function createErrorEmbed() {
    return new EmbedBuilder()
        .setTitle("No Maintenance Info")
        .setDescription("There is no maintenance information.\nIf you think this is an error, please refer to the official Lodestone.")
        .setURL("https://jp.finalfantasyxiv.com/lodestone")
        .setThumbnail(CommandCategory.GAMEINFO?.image)
        .setColor(EMBED_COLORS.ERROR);
}