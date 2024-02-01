const { EmbedBuilder, ApplicationCommandOptionType } = require("discord.js");
const { EMBED_COLORS } = require("@root/config.js");
const { UTILITY } = require("@root/src/structures/CommandCategory");
const { TIMEZONE } = require("@src/data.json");
const moment = require('moment-timezone');

const choices = [
    "UTC",
    "Europe/Paris",
    "America/Los_Angeles",
    "America/Denver",
    "America/Chicago",
    "America/New_York",
    "Europe/Athens",
    "Europe/Moscow",
    "Asia/Kolkata",
    "Asia/Dubai",
    "America/Anchorage",
    "Australia/Sydney",
    "Australia/Adelaide",
    "Australia/Darwin",
    "Australia/Perth",
    "Africa/Cairo",
    "Africa/Johannesburg",
    "America/Argentina/Buenos_Aires",
    "America/Sao_Paulo",
    "Pacific/Auckland",
    "Pacific/Honolulu",
    "Asia/Kathmandu",
    "Asia/Kabul"
  ];
  

/**
 * @type {import("@structures/Command")}
 */
module.exports = {
    name: "convertstamp",
    description: "Converts a UTC timestamp to a local time format.",
    category: "UTILITY",
    botPermissions: ["EmbedLinks"],
    slashCommand: {
        enabled: true,
        options: [
            {
                name: "timestamp",
                description: "The UTC timestamp to convert (e.g., 1720303200)",
                required: true,
                type: ApplicationCommandOptionType.String,
            },
            {
                name: "timezone",
                description: "The timezone",
                required: false,
                type: ApplicationCommandOptionType.String,
                choices: choices.map((choice) => ({ name: TIMEZONE[choice], value: choice })),
            },
        ],
    },

    async interactionRun(interaction) {
        try {
            const timestamp = interaction.options.getString("timestamp");
            const timezone = interaction.options.getString("timezone") || "Asia/Seoul";

            // Parse the timestamp using moment-timezone
            const timeMoment = moment.tz(timestamp, 'UTC');

            // Convert to the local time format based on the chosen timezone
            const localTime = moment.tz(moment.unix(timestamp), timezone).format('YYYY-MM-DD HH:mm:ss [GMT]Z');

            // Create the embed with the time details
            const embed = new EmbedBuilder()
                .setTitle('Timestamp Conversion')
                .setColor(EMBED_COLORS.BOT_EMBED)
                .addFields(
                    { name: 'UTC Timestamp', value: timestamp, inline: false },
                    { name: `Local Time (${timezone})`, value: localTime, inline: false },
                )
                .setThumbnail(UTILITY.image)
                .setTimestamp(Date.now());

            await interaction.followUp({ embeds: [embed] });
        } catch (err) {
            console.error(err);
            await interaction.followUp('An error occurred while converting the timestamp.');
        }
    }
};
