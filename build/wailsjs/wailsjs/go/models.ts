export namespace doc {
	
	export class ChunkInfo {
	    bytes: number;
	    lines: number;
	
	    static createFrom(source: any = {}) {
	        return new ChunkInfo(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.bytes = source["bytes"];
	        this.lines = source["lines"];
	    }
	}
	export class Heading {
	    level: number;
	    text: string;
	    id: string;
	    chunk: number;
	
	    static createFrom(source: any = {}) {
	        return new Heading(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.level = source["level"];
	        this.text = source["text"];
	        this.id = source["id"];
	        this.chunk = source["chunk"];
	    }
	}

}

export namespace session {
	
	export class Anchor {
	    chunk: number;
	    frac: number;
	
	    static createFrom(source: any = {}) {
	        return new Anchor(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.chunk = source["chunk"];
	        this.frac = source["frac"];
	    }
	}
	export class TabState {
	    anchor: Anchor;
	    tocScroll: number;
	    webImages: boolean;
	    webDismissed: boolean;
	
	    static createFrom(source: any = {}) {
	        return new TabState(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.anchor = this.convertValues(source["anchor"], Anchor);
	        this.tocScroll = source["tocScroll"];
	        this.webImages = source["webImages"];
	        this.webDismissed = source["webDismissed"];
	    }
	
		convertValues(a: any, classs: any, asMap: boolean = false): any {
		    if (!a) {
		        return a;
		    }
		    if (a.slice && a.map) {
		        return (a as any[]).map(elem => this.convertValues(elem, classs));
		    } else if ("object" === typeof a) {
		        if (asMap) {
		            for (const key of Object.keys(a)) {
		                a[key] = new classs(a[key]);
		            }
		            return a;
		        }
		        return new classs(a);
		    }
		    return a;
		}
	}
	export class Offer {
	    token: string;
	    info?: store.Info;
	    state: TabState;
	    index: number;
	
	    static createFrom(source: any = {}) {
	        return new Offer(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.token = source["token"];
	        this.info = this.convertValues(source["info"], store.Info);
	        this.state = this.convertValues(source["state"], TabState);
	        this.index = source["index"];
	    }
	
		convertValues(a: any, classs: any, asMap: boolean = false): any {
		    if (!a) {
		        return a;
		    }
		    if (a.slice && a.map) {
		        return (a as any[]).map(elem => this.convertValues(elem, classs));
		    } else if ("object" === typeof a) {
		        if (asMap) {
		            for (const key of Object.keys(a)) {
		                a[key] = new classs(a[key]);
		            }
		            return a;
		        }
		        return new classs(a);
		    }
		    return a;
		}
	}

}

export namespace settings {
	
	export class Settings {
	    theme: string;
	    wrap: boolean;
	    fontSize: number;
	    lineHeight: number;
	    padding: number;
	    font: string;
	    liveReload: boolean;
	    toc: boolean;
	
	    static createFrom(source: any = {}) {
	        return new Settings(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.theme = source["theme"];
	        this.wrap = source["wrap"];
	        this.fontSize = source["fontSize"];
	        this.lineHeight = source["lineHeight"];
	        this.padding = source["padding"];
	        this.font = source["font"];
	        this.liveReload = source["liveReload"];
	        this.toc = source["toc"];
	    }
	}

}

export namespace store {
	
	export class Info {
	    id: number;
	    key: number;
	    web: string;
	    path: string;
	    name: string;
	    base: string;
	    size: number;
	    parseMs: number;
	    chunks: doc.ChunkInfo[];
	    headings: doc.Heading[];
	    anchors: Record<string, number>;
	
	    static createFrom(source: any = {}) {
	        return new Info(source);
	    }
	
	    constructor(source: any = {}) {
	        if ('string' === typeof source) source = JSON.parse(source);
	        this.id = source["id"];
	        this.key = source["key"];
	        this.web = source["web"];
	        this.path = source["path"];
	        this.name = source["name"];
	        this.base = source["base"];
	        this.size = source["size"];
	        this.parseMs = source["parseMs"];
	        this.chunks = this.convertValues(source["chunks"], doc.ChunkInfo);
	        this.headings = this.convertValues(source["headings"], doc.Heading);
	        this.anchors = source["anchors"];
	    }
	
		convertValues(a: any, classs: any, asMap: boolean = false): any {
		    if (!a) {
		        return a;
		    }
		    if (a.slice && a.map) {
		        return (a as any[]).map(elem => this.convertValues(elem, classs));
		    } else if ("object" === typeof a) {
		        if (asMap) {
		            for (const key of Object.keys(a)) {
		                a[key] = new classs(a[key]);
		            }
		            return a;
		        }
		        return new classs(a);
		    }
		    return a;
		}
	}

}

