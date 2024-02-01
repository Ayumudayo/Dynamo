const { ApplicationCommandOptionType } = require("discord.js");
const fs = require('fs').promises;
const path = require('path');

/**
 * Command module for reloading bot commands.
 * @module reload
 * @property {string} name - The name of the command.
 * @property {string} description - A brief description of the command.
 * @property {string} category - The category this command belongs to.
 * @property {Array} botPermissions - Permissions the bot requires.
 * @property {Object} command - Settings for the normal command.
 * @property {Object} slashCommand - Settings for the slash command.
 */
module.exports = {
    name: "reload",
    description: "Reload command",
    category: "OWNER",
    botPermissions: ["EmbedLinks"],
    command: {
        enabled: false,
        usage: "[command]",
    },
    slashCommand: {
        enabled: true,
        options: [
            {
                name: "command",
                description: "Input name of command, If not entered, reloads all commands.",
                type: ApplicationCommandOptionType.String,
                required: false,
            },
        ],
    },

    /**
     * Handler for running the command through a message.
     * @param {Message} message - The message object from Discord.js.
     * @param {Array<string>} args - The arguments provided with the command.
     */
    async messageRun(message, args) {
        // ...
    },

    /**
     * Handler for running the command through an interaction.
     * @param {CommandInteraction} interaction - The interaction object from Discord.js.
     */
    async interactionRun(interaction) {
        const cmdName = interaction.options.getString("command");
        if (!cmdName) {
            await reloadAllCommands(interaction);
        } else {
            await reloadCommand(interaction, cmdName);
        }
    }
};

/**
 * Reloads all commands by iterating through the command files and re-importing them.
 * @param {CommandInteraction} interaction - The interaction object from Discord.js.
 */
async function reloadAllCommands(interaction) {
    const directoryPath = path.join(__dirname, '..'); // parent directory
    let failedCommands = []; // Array to keep track of failed command reloads

    try {
        const directories = await fs.readdir(directoryPath, { withFileTypes: true });
        for (const dir of directories) {
            if (dir.isDirectory()) {
                const files = await fs.readdir(path.join(directoryPath, dir.name));
                for (const file of files) {
                    if (file.endsWith('.js')) {
                        try {
                            await reloadFile(interaction, path.join(directoryPath, dir.name, file));
                        } catch (error) {
                            // If an error occurs, log it and add the command to the failedCommands array
                            console.error(error);
                            failedCommands.push(file);
                        }
                    }
                }
            }
        }

        if (failedCommands.length === 0) {
            await interaction.followUp(`All commands were reloaded successfully!`);
        } else {
            // If there were any failed commands, report them back to the user
            await interaction.followUp(`All commands reloaded, except for the following: \n\`${failedCommands.join(', ')}\`\nPlease check the logs for more details.`);
        }
    } catch (error) {
        console.error(error);
        await interaction.followUp(`There was an error while reloading commands:\n\`${error.message}\``);
    }
}


/**
 * Reloads a specific command by finding its file and re-importing it.
 * @param {CommandInteraction} interaction - The interaction object from Discord.js.
 * @param {string} cmdName - The name of the command to reload.
 */
async function reloadCommand(interaction, cmdName) {
    const cmd = interaction.client.slashCommands.get(cmdName);
    if (!cmd) {
        await interaction.followUp(`There is no command with name \`${cmdName}\`!`);
        return;
    }

    const directoryPath = path.join(__dirname, '..'); // parent directory
    try {
        const commandPath = await findCommandFile(directoryPath, cmd.name);
        if (!commandPath) {
            await interaction.followUp(`No file found for command \`${cmd.name}\`.`);
            return;
        }

        await reloadFile(interaction, commandPath);
        await interaction.followUp(`Command \`${cmd.name}\` was reloaded!`);
    } catch (error) {
        console.error(error);
        await interaction.followUp(`There was an error while reloading a command \`${cmd.name}\`:\n\`${error.message}\``);
    }
}

/**
 * Finds the file path for a given command name.
 * @param {string} directoryPath - The base directory to search in.
 * @param {string} commandName - The name of the command to find.
 * @returns {Promise<string|null>} The path to the command file or null if not found.
 */
async function findCommandFile(directoryPath, commandName) {
    const directories = await fs.readdir(directoryPath, { withFileTypes: true });
    for (const dir of directories) {
        if (dir.isDirectory()) {
            const files = await fs.readdir(path.join(directoryPath, dir.name));
            if (files.includes(`${commandName}.js`)) {
                return path.join(directoryPath, dir.name, `${commandName}.js`);
            }
        }
    }
    return null;
}

/**
 * Reloads a command file by clearing the require cache and re-importing the command.
 * @param {CommandInteraction} interaction - The interaction object from Discord.js.
 * @param {string} filePath - The path to the command file to reload.
 */
async function reloadFile(interaction, filePath) {
    try {
        delete require.cache[require.resolve(filePath)];
        const newCommand = require(filePath);
        // Remove the old command from the collection before re-adding
        interaction.client.slashCommands.delete(newCommand.name);
        interaction.client.slashCommands.set(newCommand.name, newCommand);
    } catch (error) {
        console.error(error);
        await interaction.followUp(`There was an error while reloading command from file \`${path.basename(filePath)}\`:\n\`${error.message}\``);
    }
}