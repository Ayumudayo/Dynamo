const { STOCK } = require("@root/config.js");
const Logger = require("@helpers/Logger");
const { canStartManualRefresh, getSession, restartSessionFromButton } = require("@helpers/StockRefreshSession");

async function handleRefreshButton(interaction) {
  const session = getSession(interaction.message.id);

  if (!session) {
    return interaction.reply({
      content: "This refresh session has expired. Please run `/stock` or `/etf` again.",
      ephemeral: true,
    });
  }

  if (session.active) {
    return interaction.reply({
      content: "The default refresh loop is still running, so this button is not available yet.",
      ephemeral: true,
    });
  }

  if (session.manualRestartInProgress) {
    return interaction.reply({
      content: "A refresh restart is already being prepared for this message.",
      ephemeral: true,
    });
  }

  if (!canStartManualRefresh(session)) {
    return interaction.reply({
      content: `You can manually restart this refresh loop up to ${STOCK.MAX_MANUAL_REFRESHES} times.`,
      ephemeral: true,
    });
  }

  session.manualRestartInProgress = true;

  try {
    await interaction.deferUpdate();

    const response = await restartSessionFromButton(interaction, session);
    if (response?.embed) return;

    return interaction.followUp({
      content: "Failed to refresh stock data. Please try again later.",
      ephemeral: true,
    });
  } catch (error) {
    Logger.error("stock handleRefreshButton", error);

    return interaction
      .followUp({
        content: "Failed to refresh stock data. Please try again later.",
        ephemeral: true,
      })
      .catch(() => {});
  } finally {
    session.manualRestartInProgress = false;
  }
}

module.exports = {
  handleRefreshButton,
};
