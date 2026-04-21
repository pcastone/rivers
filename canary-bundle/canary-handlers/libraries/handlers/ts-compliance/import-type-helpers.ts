// Helper for RT-TS-IMPORT-TYPE — exports both a type and a runtime value.

export type Answer = { value: number };

export function buildAnswer(n: number): Answer {
    return { value: n };
}
