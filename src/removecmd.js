require("dotenv").config();
const { REST, Routes } = require('discord.js');

// Construct and prepare an instance of the REST module
const rest = new REST().setToken(process.env.BOT_TOKEN);

const clientId = process.env.BOT_CLIENT_ID;
const guildId = '';
const cmdId = '';

console.log(clientId)
console.log(guildId)

// for guild-based commands
// rest.delete(Routes.applicationGuildCommand(clientId, guildId, cmdId))
// 	.then(() => console.log('Successfully deleted guild command'))
// 	.catch(console.error);

// for global commands
rest.put(Routes.applicationCommands(clientId), { body: [] })
	.then(() => console.log('Successfully deleted all application commands.'))
	.catch(console.error);