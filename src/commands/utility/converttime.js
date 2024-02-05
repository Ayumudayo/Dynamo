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
    name: "converttime",
    description: "Converts a given time to a UTC Timestamp and extracts date components.",
    category: "UTILITY",
    botPermissions: ["EmbedLinks"],
    slashCommand: {
        enabled: true,
        options: [            
            {
                name: "month",
                description: "The month (1-12)",
                required: true,
                type: ApplicationCommandOptionType.Integer,
            },
            {
                name: "day",
                description: "The day (1-31)",
                required: true,
                type: ApplicationCommandOptionType.Integer,
            },
            {
                name: "hour",
                description: "The hour (0-23)",
                required: true,
                type: ApplicationCommandOptionType.Integer,
                minValue: 0,
                maxValue: 23,
            },
            {
                name: "minute",
                description: "The minute (0-59)",
                required: false,
                type: ApplicationCommandOptionType.Integer,
                minValue: 0,
                maxValue: 59,
            },
            {
                name: "second",
                description: "The second (0-59)",
                required: false,
                type: ApplicationCommandOptionType.Integer,
                minValue: 0,
                maxValue: 59,
            },
            {
                name: "year",
                description: "The year",
                required: false,
                type: ApplicationCommandOptionType.Integer,
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
            const hour = interaction.options.getInteger("hour");
            const day = interaction.options.getInteger("day");
            const month = interaction.options.getInteger("month");
            const year = interaction.options.getInteger("year") || new Date().getFullYear();
            const minute = interaction.options.getInteger("minute") || 0;
            const second = interaction.options.getInteger("second") || 0;
            const timezone = interaction.options.getString("timezone") || "Asia/Seoul";

            // Construct the time string and parse it using moment-timezone
            const timeString = `${year}-${month}-${day} ${hour}:${minute}:${second}`;
            const timeMoment = moment.tz(timeString, 'YYYY-MM-DD HH:mm:ss', timezone);

            // Check if the time is valid
            if (!timeMoment.isValid()) {
                await interaction.followUp('The provided time is invalid.');
                return;
            }

            // Convert to UTC timestamp
            const utcTimestamp = timeMoment.unix().toString()

            // Create the embed with the time details
            const embed = new EmbedBuilder()
                .setTitle('Time Conversion')
                .setColor(EMBED_COLORS.BOT_EMBED)
                .addFields(
                    { name: `Local Time (${timezone})`, value: timeMoment.format('YYYY-MM-DD HH:mm:ss [GMT]Z'), inline: false },
                    { name: 'UTC Timestamp', value: utcTimestamp, inline: false },
                )
                .setThumbnail(UTILITY.image)
                .setTimestamp(Date.now());

            await interaction.followUp({ embeds: [embed] });
        } catch (err) {
            console.error(err);
            await interaction.followUp('An error occurred while converting the time.');
        }
    }
};