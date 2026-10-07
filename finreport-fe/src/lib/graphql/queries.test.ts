import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import path from 'node:path';
import {
	buildSchema,
	isNonNullType,
	parse,
	TypeInfo,
	validate,
	visit,
	visitWithTypeInfo,
	type OperationDefinitionNode
} from 'graphql';
import * as queries from './queries';
import * as adminReviewQueries from './adminReviewQueries';
import * as adminRulesQueries from './adminRulesQueries';

/**
 * Validates every operation the app sends against the frozen SDL, plus one
 * stricter rule the GraphQL spec allows but async-graphql rejects at runtime:
 * a nullable variable without a default feeding a non-null argument that has a
 * schema default (e.g. `$x: Boolean` -> `arg: Boolean! = false`). Omitting
 * such a variable makes async-graphql pass `null` and fail the whole query.
 */

const schema = buildSchema(readFileSync(path.join(__dirname, 'schema.graphql'), 'utf8'));

const exported: Record<string, unknown> = {
	...queries,
	...adminReviewQueries,
	...adminRulesQueries
};
const documents = Object.entries(exported)
	.filter((entry): entry is [string, string] => typeof entry[1] === 'string')
	.filter(([, value]) => /^\s*(query|mutation|subscription)\b/.test(value));

function nullableVariablesInNonNullPositions(source: string): string[] {
	const document = parse(source);
	const problems: string[] = [];
	const typeInfo = new TypeInfo(schema);
	for (const definition of document.definitions) {
		if (definition.kind !== 'OperationDefinition') continue;
		const op = definition as OperationDefinitionNode;
		const risky = new Set(
			(op.variableDefinitions ?? [])
				.filter((v) => v.type.kind !== 'NonNullType' && !v.defaultValue)
				.map((v) => v.variable.name.value)
		);
		visit(
			op,
			visitWithTypeInfo(typeInfo, {
				Argument(node) {
					const arg = typeInfo.getArgument();
					if (
						node.value.kind === 'Variable' &&
						risky.has(node.value.name.value) &&
						arg &&
						isNonNullType(arg.type)
					) {
						problems.push(`$${node.value.name.value} -> ${arg.name}: ${String(arg.type)}`);
					}
				}
			})
		);
	}
	return problems;
}

describe('GraphQL operations', () => {
	it('finds operations to check', () => {
		expect(documents.length).toBeGreaterThan(20);
	});

	it.each(documents)('%s validates against the schema', (_name, source) => {
		expect(validate(schema, parse(source)).map((e) => e.message)).toEqual([]);
	});

	it.each(documents)('%s passes no nullable variable to a non-null argument', (_name, source) => {
		expect(nullableVariablesInNonNullPositions(source)).toEqual([]);
	});
});
