/* eslint-disable @typescript-eslint/no-explicit-any -- the generated documents are free-form JSON */
export class SchemaError extends Error {}

export const HEADER: string;

export interface Definition {
  name: string;
  file: string;
  schema: Record<string, unknown>;
}

export type Definitions = Map<string, Definition>;

export function pascalCase(value: string): string;
export function collect(documents: Record<string, unknown>): Definitions;
export function bundle(definitions: Definitions): {
  $schema: string;
  $id: string;
  $defs: Record<string, any>;
};
export function typescript(definitions: Definitions): string;
export function rust(
  definitions: Definitions,
  options?: {
    include?: (definition: Definition) => boolean;
    imports?: string;
  },
): string;
export function openapi(
  definitions: Definitions,
  routes: { routes: unknown[] },
): any;
export function client(routes: { routes: unknown[] }): string;
