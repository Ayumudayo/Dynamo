require("dotenv").config();

module.exports = {
  OWNER_IDS: [process.env.OWNER_ID], // Bot owner ID's
  SUPPORT_SERVER: "", // Your bot support server
  PREFIX_COMMANDS: {
    ENABLED: true, // Enable/Disable prefix commands
    DEFAULT_PREFIX: "!", // Default prefix for the bot
  },
  INTERACTIONS: {
    SLASH: true, // Should the interactions be enabled
    CONTEXT: true, // Should contexts be enabled
    GLOBAL: true, // Should the interactions be registered globally
    TEST_GUILD_ID: process.env.GUILD_ID, // Guild ID where the interactions should be registered. [** Test you commands here first **]
  },
  EMBED_COLORS: {
    BOT_EMBED: "#068ADD",
    TRANSPARENT: "#36393F",
    SUCCESS: "#00A56A",
    ERROR: "#D61A3C",
    WARNING: "#F7E919",
  },
  CACHE_SIZE: {
    GUILDS: 100,
    USERS: 10000,
    MEMBERS: 10000,
  },
  MESSAGES: {
    API_ERROR: "Unexpected Backend Error! Try again later or contact support server",
  },

  // PLUGINS

  AUTOMOD: {
    ENABLED: true,
    LOG_EMBED: "#36393F",
    DM_EMBED: "#36393F",
  },

  DASHBOARD: {
    enabled: true, // enable or disable dashboard
    baseURL: "http://localhost:8080", // base url
    failureURL: "http://localhost:8080", // failure redirect url
    port: "8080", // port to run the bot on
  },

  CURRENCY: {
    enabled: true,
  },

  ECONOMY: {
    ENABLED: false,
    CURRENCY: "₪",
    DAILY_COINS: 100, // coins to be received by daily command
    MIN_BEG_AMOUNT: 100, // minimum coins to be received when beg command is used
    MAX_BEG_AMOUNT: 2500, // maximum coins to be received when beg command is used
  },

  MUSIC: {
    ENABLED: true,
    IDLE_TIME: 60, // Time in seconds before the bot disconnects from an idle voice channel
    MAX_SEARCH_RESULTS: 5,
    DEFAULT_SOURCE: "YT", // YT = Youtube, YTM = Youtube Music, SC = SoundCloud
    // Add any number of lavalink nodes here
    // Refer to https://github.com/freyacodes/Lavalink to host your own lavalink server
    LAVALINK_NODES: [      
      {
        host: "127.0.0.1",
        port: 23333,
        password: "kabirjaipal",
        id: "render",
        secure: false,
      },
      {
        host: "3.34.197.204",
        port: 2333,
        password: "kabirjaipal",
        id: "amazon",
        secure: false,
      },
      {
        host: "lava.alfari.id",
        port: 443,
        password: "youshallnotpass",
        id: "alfari.id",
        secure: true,
      },
      {
        host: "lavalink.lexnet.cc",
        port: 2333,
        password: "lexn3tl@val!nk",
        id: "lex",
        secure: false,
      },
      {
        host: "eu-lavalink.lexnet.cc",
        port: 2333,
        password: "lexn3tl@val!nk",
        id: "lex-eu",
        secure: false,
      },
      {
        host: "bali.weissx.com",
        port: 14007,
        password: "youshallnotpass",
        id: "weiss",
        secure: false,
      },
      {
        host: "lavalink.teramont.net",
        port: 443,
        password: "eHKuFcz67k4lBS64",
        id: "teramont",
        secure: true,
      },
    ],
  },

  GAMEINFO: {
    ENABLED: true
  },

  GIVEAWAYS: {
    ENABLED: true,
    REACTION: "🎁",
    START_EMBED: "#FF468A",
    END_EMBED: "#FF468A",
  },

  IMAGE: {
    ENABLED: false,
    BASE_API: "https://strangeapi.fun/api",
  },

  INVITE: {
    ENABLED: false,
  },

  MODERATION: {
    ENABLED: true,
    EMBED_COLORS: {
      TIMEOUT: "#102027",
      UNTIMEOUT: "#4B636E",
      KICK: "#FF7961",
      SOFTBAN: "#AF4448",
      BAN: "#D32F2F",
      UNBAN: "#00C853",
      VMUTE: "#102027",
      VUNMUTE: "#4B636E",
      DEAFEN: "#102027",
      UNDEAFEN: "#4B636E",
      DISCONNECT: "RANDOM",
      MOVE: "RANDOM",
    },
  },

  PRESENCE: {
    ENABLED: false, // Whether or not the bot should update its status
    STATUS: "online", // The bot's status [online, idle, dnd, invisible]
    TYPE: "WATCHING", // Status type for the bot [PLAYING | LISTENING | WATCHING | COMPETING]
    MESSAGE: "{members} members in {servers} servers", // Your bot status message
  },

  STATS: {
    ENABLED: false,
    XP_COOLDOWN: 5, // Cooldown in seconds between messages
    DEFAULT_LVL_UP_MSG: "{member:tag}, You just advanced to **Level {level}**",
  },

  STOCK: {
    ENABLED: true,
    DEFAULT_EMBED: "#4F545C",
    UPWARD_EMBED: "#43B581",
    DOWNWARD_EMBED: "#F04747",
    REFRESH_INTERVAL: 5000, // ms
    MAX_REFRESH_TIME: 60000, // ms
  },

  SUGGESTIONS: {
    ENABLED: true, // Should the suggestion system be enabled
    EMOJI: {
      UP_VOTE: "⬆️",
      DOWN_VOTE: "⬇️",
    },
    DEFAULT_EMBED: "#4F545C",
    APPROVED_EMBED: "#43B581",
    DENIED_EMBED: "#F04747",
  },

  TICKET: {
    ENABLED: true,
    CREATE_EMBED: "#068ADD",
    CLOSE_EMBED: "#068ADD",
  },
};
