import { describe, expect, it } from 'vitest';
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { buildSchema, parse, validate } from 'graphql';

/**
 * Validates every GraphQL mock under `mocks/` against the frozen SDL
 * (`schema.graphql`, mirrored verbatim from `finreport-rs/webapp/schema.graphql`
 * by WP0 — see docs/specs/iteration-1.md §5 and §9). This is the FE-side half
 * of the "schema.graphql parses and mocks validate against it" WP0 exit
 * criterion: it catches a mock's query drifting from the SDL (typo'd field,
 * removed argument, renamed type) without needing a live backend.
 *
 * Each mock file is a JSON document of the shape `{ query, variables?, data }`
 * — `query` is executed with `validate()` against the schema; a structurally
 * invalid query (not merely a backend behaviour mismatch) fails the test.
 */

const graphqlDir = path.resolve(__dirname);
const schemaPath = path.join(graphqlDir, 'schema.graphql');
const mocksDir = path.join(graphqlDir, 'mocks');

const schema = buildSchema(readFileSync(schemaPath, 'utf-8'));

const mockFiles = readdirSync(mocksDir).filter((name) => name.endsWith('.json'));

describe('schema.graphql', () => {
	it('parses as a valid GraphQL SDL document', () => {
		expect(schema).toBeDefined();
	});
});

describe('graphql mocks', () => {
	it('found at least one mock fixture', () => {
		expect(mockFiles.length).toBeGreaterThan(0);
	});

	for (const file of mockFiles) {
		it(`${file}: query validates against schema.graphql`, () => {
			const raw = readFileSync(path.join(mocksDir, file), 'utf-8');
			const mock = JSON.parse(raw);

			expect(mock).toHaveProperty('query');
			expect(mock).toHaveProperty('data');

			const document = parse(mock.query as string);
			const errors = validate(schema, document);

			expect(errors, errors.map((e) => e.message).join('\n')).toHaveLength(0);
		});
	}
});
