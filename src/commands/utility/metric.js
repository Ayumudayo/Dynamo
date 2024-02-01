const { CommandCategory } = require("@src/structures");
const { EMBED_COLORS } = require("@root/config.js");
const { EmbedBuilder, ApplicationCommandOptionType } = require("discord.js");
const { IMP_UNIT } = require("@src/data.json");

const choices = ["in", "ft", "yd", "mi", "gal", "oz", "lb", "°F"];

const CONVERSIONS = {
    in: { factor: 2.54, unit: "cm" },
    ft: { factor: 0.3048, unit: "m" },
    yd: { factor: 0.9144, unit: "m" },
    mi: { factor: 1.60934, unit: "Km" },
    gal: { factor: 3.78541, unit: "L" },
    oz: { factor: 28.3495, unit: "g" },
    lb: { factor: 0.453592, unit: "Kg" },
    "°F": { factor: 5 / 9, offset: -32, unit: "°C" },
};

/**
 * @type {import("@structures/Command")}
 */
module.exports = {
    name: "metric",
    description: "Convert imperial to METRIC.",
    category: "UTILITY",
    botPermissions: ["EmbedLinks"],
    command: {
        enabled: false,
        usage: "[command]",
    },
    slashCommand: {
        enabled: true,
        options: [
            {
                name: "imperial",
                description: "The imperial units you want to convert",
                required: true,
                type: ApplicationCommandOptionType.String,
                choices: choices.map((choice) => ({ name: IMP_UNIT[choice], value: choice })),
            },
            {
                name: "amount",
                description: "Amount of unit",
                required: false,
                type: ApplicationCommandOptionType.Integer,
            },
        ],
    },

    async messageRun(message, args) {
        // ...
    },

    async interactionRun(interaction) {
        const unit = interaction.options.getString("imperial");
        const amount = interaction.options.getInteger("amount") || 1;

        const embed = await getResultEmbed(unit, amount);
        interaction.followUp({ embeds: [embed] });
    },
};

async function getResultEmbed(unit, amount) {
    const result = imperialToMetric(unit, amount);
    const conversionRate = imperialToMetric(unit, 1);

    const embed = new EmbedBuilder()
        .setTitle("Imperial to Metric Conversion")
        .setThumbnail(CommandCategory.UTILITY?.image)
        .setTimestamp(Date.now());

    if (!result) {
        embed
            .setDescription("Something went wrong. Please check if you entered an invalid unit.")
            .setColor(EMBED_COLORS.ERROR);
    } else {
        embed
            .setDescription(
                `**${amount.toLocaleString({ maximumFractionDigits: 2 })} ${unit}** is approximately \n` +
                `**${result.value.toLocaleString({ maximumFractionDigits: 2 })} ${result.unit}**\n\n\n` +
                `Conversion Rate: **1 ${unit}** = **${conversionRate.value.toLocaleString({ maximumFractionDigits: 2 })} ${conversionRate.unit}**`
            )
            .setColor(EMBED_COLORS.BOT_EMBED);
    }

    return embed;
}

function imperialToMetric(unit, value) {
    const conversion = CONVERSIONS[unit];
    if (!conversion) return null;

    let convertedValue = conversion.factor * value;
    if (conversion.offset !== undefined) {
        convertedValue = conversion.factor * (value + conversion.offset);
    }

    return { value: convertedValue, unit: conversion.unit };
}
