export function EventsOn(eventName: string, callback: (payload: any) => void): () => void;
export function EventsOnMultiple(
  eventName: string,
  callback: (payload: any) => void,
  maxCallbacks: number,
): () => void;
export function EventsOnce(eventName: string, callback: (payload: any) => void): () => void;
export function EventsOff(eventName?: string): void;
export function EventsOffAll(): void;
export function EventsEmit(eventName: string, payload?: any): never;
