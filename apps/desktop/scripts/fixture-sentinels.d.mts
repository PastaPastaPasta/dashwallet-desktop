// Types of fixture-sentinels.mjs for src/fixture/sentinels.test.ts.
export declare const MIN_LENGTH: number;
export declare const SHARED: string[];
export declare function sources(app: string): Generator<[path: string, isFixture: boolean]>;
export declare function literalsOf(path: string): Promise<Set<string>>;
export declare function overlap(app: string): Promise<{ fixture: Map<string, string>; shared: string[] }>;
export declare function fixtureSentinels(app: string): Promise<Map<string, string>>;
export declare function holds(text: string, sentinel: string): boolean;
export declare const GENERIC: string[];
