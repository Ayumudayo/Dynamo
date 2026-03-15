const fs = require("node:fs/promises");
const path = require("node:path");
const { randomUUID } = require("node:crypto");

const writeQueues = new Map();

function getQueueKey(filePath) {
  return path.resolve(filePath);
}

function enqueue(filePath, task) {
  const queueKey = getQueueKey(filePath);
  const previous = writeQueues.get(queueKey) || Promise.resolve();
  let release;
  const next = new Promise((resolve) => {
    release = resolve;
  });

  writeQueues.set(queueKey, next);

  const runTask = async () => {
    await previous;
    return task();
  };

  return runTask().finally(() => {
    release();
    if (writeQueues.get(queueKey) === next) {
      writeQueues.delete(queueKey);
    }
  });
}

async function readJson(filePath) {
  try {
    const data = await fs.readFile(filePath, "utf-8");
    return JSON.parse(data);
  } catch (error) {
    if (error.code === "ENOENT") return {};

    console.error("Error loading JSON data:", error);
    return {};
  }
}

async function writeJsonAtomically(filePath, data) {
  const directory = path.dirname(filePath);
  const fileName = path.basename(filePath);
  const tempFilePath = path.join(directory, `${fileName}.${randomUUID()}.tmp`);
  const payload = JSON.stringify(data, null, 2);

  const handle = await fs.open(tempFilePath, "w");

  try {
    await handle.writeFile(payload, "utf-8");
    await handle.sync();
  } finally {
    await handle.close();
  }

  try {
    await fs.rename(tempFilePath, filePath);
  } catch (error) {
    await fs.unlink(tempFilePath).catch(() => {});
    throw error;
  }
}

function createJsonFileStore(filePath) {
  const resolvedPath = path.resolve(filePath);

  return {
    async load() {
      return readJson(resolvedPath);
    },

    async update(updateValue) {
      return enqueue(resolvedPath, async () => {
        const current = await readJson(resolvedPath);
        const merged = typeof updateValue === "function" ? await updateValue(current) : { ...current, ...updateValue };

        const nextData = merged && typeof merged === "object" ? merged : current;

        await writeJsonAtomically(resolvedPath, nextData);
        return nextData;
      });
    },
  };
}

module.exports = {
  createJsonFileStore,
};
