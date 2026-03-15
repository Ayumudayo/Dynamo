const fs = require("node:fs/promises");
const path = require("node:path");
const staticGameInfoData = require("@src/data.json");

const { createJsonFileStore } = require("./JsonFileStore");

const dataFilePath = path.resolve(__dirname, "../data/gameinfo-cache.json");
const sampleDataFilePath = path.resolve(__dirname, "../data/gameinfo-cache.sample.json");
const runtimeDataStore = createJsonFileStore(dataFilePath);
const RUNTIME_KEYS = ["MAINTINFO", "PLLINFO"];

let ensureCacheFilePromise = null;

async function ensureCacheFile() {
  if (ensureCacheFilePromise) return ensureCacheFilePromise;

  ensureCacheFilePromise = (async () => {
    try {
      await fs.access(dataFilePath);
      return;
    } catch (error) {
      if (error.code !== "ENOENT") {
        throw error;
      }
    }

    await fs.mkdir(path.dirname(dataFilePath), { recursive: true });

    try {
      await fs.copyFile(sampleDataFilePath, dataFilePath);
    } catch (error) {
      if (error.code === "ENOENT") {
        await fs.writeFile(dataFilePath, "{}\n", "utf-8");
      } else {
        throw error;
      }
    }
  })();

  try {
    await ensureCacheFilePromise;
  } finally {
    ensureCacheFilePromise = null;
  }
}

function pickRuntimeData(data) {
  return RUNTIME_KEYS.reduce((acc, key) => {
    if (Object.hasOwn(data || {}, key)) {
      acc[key] = data[key];
    }

    return acc;
  }, {});
}

function mergeStaticAndRuntime(runtimeData) {
  return {
    ...staticGameInfoData,
    ...pickRuntimeData(runtimeData),
  };
}

module.exports = {
  async load() {
    await ensureCacheFile();

    const runtimeData = await runtimeDataStore.load();
    return mergeStaticAndRuntime(runtimeData);
  },

  async update(updateValue) {
    await ensureCacheFile();

    const runtimeData = await runtimeDataStore.update(async (currentRuntimeData) => {
      const currentData = mergeStaticAndRuntime(currentRuntimeData);
      const nextData =
        typeof updateValue === "function" ? await updateValue(currentData) : { ...currentData, ...updateValue };

      return pickRuntimeData(nextData);
    });

    return mergeStaticAndRuntime(runtimeData);
  },
};
