// Types of fixture-sentinels.mjs for src/fixture/sentinels.test.ts.
export declare const MIN_LENGTH: number;
export declare const EXTENSIONS: Record<string, "ts" | "tsx" | "js" | "jsx">;
export declare const GENERATED: string;
export declare const TEST_DATA: Record<string, string>;
export declare const NO_SENTINEL: Record<string, string>;
export declare const GENERIC: { literal: string; reason: string }[];
export declare const IGNORED: RegExp;
export declare function fixtureModules(app: string): Generator<string>;
export declare function fixtureTestData(app: string): Generator<string>;
export declare const DATA_TYPES: Record<string, (text: string) => Set<string>>;
export declare function literalsOf(path: string, options?: { types?: boolean }): Promise<Set<string>>;
export declare function distinctive(literal: string): boolean;
export declare function generatedLiterals(app: string): Promise<Set<string>>;
export declare function fixtureSentinels(app: string): Promise<{ sentinels: Map<string, string>; bySource: Map<string, string[]>; bundled: Set<string> }>;
export declare function holds(text: string, sentinel: string): boolean;
