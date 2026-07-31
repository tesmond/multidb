const callbacks = new Map();
const listeners = new Map();
let nextId = 1;
const DEFAULT_IPC_TIMEOUT_MS = 20000;

function timeoutForCommand(command) {
  switch (command) {
    case "test_connection":
      return 20000;
    case "execute_query":
    case "execute_query_streamed":
      return 0;
    default:
      return DEFAULT_IPC_TIMEOUT_MS;
  }
}

function ensureHost() {
  if (!window.ipc || typeof window.ipc.postMessage !== "function") {
    throw new Error("multidb desktop IPC is unavailable");
  }
}

export function invoke(command, args = {}) {
  ensureHost();
  const id = String(nextId++);
  return new Promise((resolve, reject) => {
    const timeoutMs = timeoutForCommand(command);
    const timeoutId = timeoutMs > 0
      ? window.setTimeout(() => {
          const callback = callbacks.get(id);
          if (!callback) return;
          callbacks.delete(id);
          callback.reject(
            new Error(`IPC timeout after ${timeoutMs / 1000}s for command ${command}`),
          );
        }, timeoutMs)
      : null;

    callbacks.set(id, {
      resolve,
      reject,
      timeoutId,
    });
    window.ipc.postMessage(JSON.stringify({ id, command, args }));
  });
}

export function listen(eventName, callback) {
  let eventListeners = listeners.get(eventName);
  if (!eventListeners) {
    eventListeners = new Set();
    listeners.set(eventName, eventListeners);
  }
  eventListeners.add(callback);
  return () => {
    eventListeners.delete(callback);
    if (eventListeners.size === 0) listeners.delete(eventName);
  };
}

window.__MULTIDB__ = {
  resolve(id, payload) {
    const callback = callbacks.get(String(id));
    if (!callback) return;
    callbacks.delete(String(id));
    if (callback.timeoutId !== null) {
      window.clearTimeout(callback.timeoutId);
    }
    callback.resolve(payload);
  },
  reject(id, error) {
    const callback = callbacks.get(String(id));
    if (!callback) return;
    callbacks.delete(String(id));
    if (callback.timeoutId !== null) {
      window.clearTimeout(callback.timeoutId);
    }
    callback.reject(error);
  },
  emit(eventName, payload) {
    const eventListeners = listeners.get(eventName);
    if (!eventListeners) return;
    for (const callback of [...eventListeners]) {
      callback(payload);
    }
  },
};
