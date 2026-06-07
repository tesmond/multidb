import { listen } from "@tauri-apps/api/event";

export function EventsOn(eventName, callback) {
  let disposed = false;
  let unlisten = null;

  listen(eventName, ({ payload }) => {
    if (!disposed) {
      callback(payload);
    }
  }).then((fn) => {
    if (disposed) {
      fn();
    } else {
      unlisten = fn;
    }
  });

  return () => {
    disposed = true;
    if (unlisten) {
      unlisten();
    }
  };
}

export function EventsOnMultiple(eventName, callback, maxCallbacks) {
  let count = 0;
  const off = EventsOn(eventName, (payload) => {
    count += 1;
    callback(payload);
    if (maxCallbacks > -1 && count >= maxCallbacks) {
      off();
    }
  });
  return off;
}

export function EventsOnce(eventName, callback) {
  return EventsOnMultiple(eventName, callback, 1);
}

export function EventsOff() {}

export function EventsOffAll() {}

export function EventsEmit() {
  throw new Error("EventsEmit is not implemented in the Tauri compatibility runtime");
}
