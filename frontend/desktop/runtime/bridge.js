const callbacks = new Map();
const listeners = new Map();
let nextId = 1;

function ensureHost() {
  if (!window.ipc || typeof window.ipc.postMessage !== "function") {
    throw new Error("multidb desktop IPC is unavailable");
  }
}

export function invoke(command, args = {}) {
  ensureHost();
  const id = String(nextId++);
  return new Promise((resolve, reject) => {
    callbacks.set(id, { resolve, reject });
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
    callback.resolve(payload);
  },
  reject(id, error) {
    const callback = callbacks.get(String(id));
    if (!callback) return;
    callbacks.delete(String(id));
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
