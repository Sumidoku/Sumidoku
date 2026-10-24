export type SseHandler = (data: unknown) => void;
export type SseStatusHandler = (online: boolean) => void;

export class SseClient {
	private source: EventSource | null = null;
	private handlers = new Map<string, Set<SseHandler>>();
	private attached = new Set<string>();
	private statusHandlers = new Set<SseStatusHandler>();
	private online = false;

	on(name: string, handler: SseHandler): () => void {
		let set = this.handlers.get(name);
		if (!set) {
			set = new Set();
			this.handlers.set(name, set);
		}
		set.add(handler);
		if (!this.source) this.connect();
		else this.attach(name);

		return () => {
			const current = this.handlers.get(name);
			if (!current) return;
			current.delete(handler);
			if (current.size === 0) this.handlers.delete(name);
		};
	}

	onStatus(handler: SseStatusHandler): () => void {
		this.statusHandlers.add(handler);
		handler(this.online);
		return () => {
			this.statusHandlers.delete(handler);
		};
	}

	start() {
		this.connect();
	}

	stop() {
		this.disconnect();
	}

	private connect() {
		if (this.source) return;
		this.source = new EventSource('/api/v1/events');
		this.source.onopen = () => this.setOnline(true);
		this.source.onerror = () => this.setOnline(false);
		for (const name of this.handlers.keys()) this.attach(name);
	}

	private disconnect() {
		if (this.source) {
			this.source.onopen = null;
			this.source.onerror = null;
			this.source.close();
		}
		this.source = null;
		this.attached.clear();
		this.setOnline(false);
	}

	private attach(name: string) {
		if (!this.source || this.attached.has(name)) return;
		this.attached.add(name);
		this.source.addEventListener(name, (event) => {
			const raw = (event as MessageEvent).data;
			if (typeof raw !== 'string') return;
			let data: unknown;
			try {
				data = JSON.parse(raw);
			} catch {
				return;
			}
			if (!data || typeof data !== 'object' || Array.isArray(data)) return;
			this.handlers.get(name)?.forEach((handler) => handler(data));
		});
	}

	private setOnline(online: boolean) {
		if (this.online === online) return;
		this.online = online;
		this.statusHandlers.forEach((handler) => handler(online));
	}
}

export const sse = new SseClient();
