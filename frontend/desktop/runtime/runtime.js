import { listen } from "./bridge.js";

export function EventsOn(eventName, callback) {
  return listen(eventName, callback);
}

export function EventsOff() {}
