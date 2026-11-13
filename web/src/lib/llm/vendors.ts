// SPDX-License-Identifier: EPL-2.0

// Which vendor's logo a bot's declared model (`robot.llm.model`, free text) gets.

import type { Llm } from '#lib/rumble/types.ts';
import type { Vendor } from './logos.ts';

/**
 * First match wins. The Copilots come last: "GitHub Copilot (Claude Sonnet 4.5)" gets the
 * model's logo, plain "GitHub Copilot" Copilot's. Other tools (Cursor, …) get the robot.
 */
const RULES: [Vendor, RegExp][] = [
	['claude', /\b(claude|anthropic|opus|sonnet|haiku)/i],
	['openai', /\b(openai|chatgpt|gpt|codex|o[1-9]\b)/i],
	['gemma', /\bgemma/i],
	['gemini', /\b(gemini|bard)/i],
	['grok', /\b(grok|xai)/i],
	['deepseek', /deepseek/i],
	['mistral', /\b(mistral|mixtral|codestral|devstral|magistral)/i],
	['qwen', /\b(qwen|qwq)/i],
	['meta', /\b(llama|meta)\b/i],
	['kimi', /\b(kimi|moonshot)/i],
	['zhipu', /\b(glm|chatglm|zhipu)/i],
	['minimax', /minimax/i],
	['microsoft', /\bphi-?\d/i],
	['nova', /\b(amazon nova|nova (micro|lite|pro|premier))/i],
	['cohere', /\b(cohere|command[- ]?(r|a)\b)/i],
	['nvidia', /\b(nvidia|nemotron)/i],
	['ibm', /\bgranite/i],
	['wenxin', /\b(ernie|wenxin)/i],
	['hunyuan', /hunyuan/i],
	['doubao', /doubao/i],
	['perplexity', /\b(perplexity|sonar)/i],
	['githubcopilot', /github\s*copilot/i],
	['copilot', /copilot/i]
];

/** The vendor of one model name, or null where it isn't known (the site shows a robot). */
export function vendorOf(model: string): Vendor | null {
	return RULES.find(([, re]) => re.test(model))?.[0] ?? null;
}

/**
 * The icons for a declaration's models (comma separated), each vendor once, in the order
 * written, at most `max`; null stands for an unknown model, or for no model named.
 */
export function vendorIcons(model: string | null | undefined, max = 3): (Vendor | null)[] {
	const names = (model ?? '')
		.split(',')
		.map((m) => m.trim())
		.filter(Boolean);
	const icons = [...new Set(names.length ? names.map(vendorOf) : [null])];
	return icons.slice(0, max);
}

/** The hover text: "LLM-generated: Claude Opus 4.5" and the like. */
export function llmLabel(llm: Llm): string {
	if (llm.use === 'none') return 'No LLM used (declared by the author)';
	const kind = llm.use === 'generated' ? 'LLM-generated' : 'LLM-assisted';
	return llm.model ? `${kind}: ${llm.model}` : `${kind} (model not named)`;
}
