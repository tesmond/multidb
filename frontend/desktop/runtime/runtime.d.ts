export function EventsOn(eventName: string, callback: (payload: any) => void): () => void;
export function EventsOff(eventName?: string): void;
