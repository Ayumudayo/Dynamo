require("dotenv").config();
const { REST, Routes } = require('discord.js');

// Construct and prepare an instance of the REST module
const rest = new REST().setToken(process.env.BOT_TOKEN);

const clientId = process.env.BOT_CLIENT_ID;
const guildId = process.env.GUILD_ID;
const cmdId = '1203980508661547053';

// for guild-based commands
rest.delete(Routes.applicationGuildCommand(clientId, guildId, cmdId))
	.then(() => console.log('Successfully deleted guild command'))
	.catch(console.error);

// for global commands
// rest.delete(Routes.applicationCommand(clientId, 'commandId'))
// 	.then(() => console.log('Successfully deleted application command'))
// 	.catch(console.error);