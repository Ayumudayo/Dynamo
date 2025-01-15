const {CommandCategory} = require("@src/structures");
const {EMBED_COLORS} = require("@root/config.js");
const {
    EmbedBuilder,
    ApplicationCommandOptionType,
    ActionRowBuilder,
    ButtonBuilder,
    ButtonStyle,
} = require("discord.js");
const fs = require("fs/promises");
const path = require("path");

const dataFilePath = path.join(__dirname, "../../data.json");

/**
 * @type {import("@structures/Command")}
 */
module.exports = {
    name: "wtinv",
    description: "Shows the War Thunder Invite Link.",
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
        // ...
    },

    async interactionRun(interaction) {
        try {
            const INVITE_LINK = await loadData().WTINFO.link;

            const embed = new EmbedBuilder()
                .setTitle("Join Warthunder Now") // 타이틀 변경
                .setColor(EMBED_COLORS.SUCCESS)
                // .setDescription("워썬더에 합류하세요!")
                .setTimestamp();

            // [버튼] JOIN
            const buttonRow = new ActionRowBuilder().addComponents(
                new ButtonBuilder()
                    .setLabel("JOIN")
                    .setStyle(ButtonStyle.Success) // 클릭 시 링크로 연결
                    .setURL(INVITE_LINK)
            );

            await interaction.followUp({
                embeds: [embed],
                components: [buttonRow],
            });
        } catch (err) {
            console.debug(err);
            await interaction.followUp("오류가 발생했습니다.");
        }
    },
};

/**
 * Loads and returns data from a JSON file.
 * @async
 * @returns {Promise<Object>} The loaded data.
 */
async function loadData() {
    try {
        const data = await fs.readFile(dataFilePath, "utf-8");
        return JSON.parse(data);
    } catch (error) {
        return {};
    }
}
