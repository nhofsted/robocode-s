// SPDX-License-Identifier: EPL-2.0

import { describe, expect, it } from 'vitest';
import { llmLabel, vendorIcons, vendorOf } from './vendors.ts';

describe('vendorOf', () => {
	it('knows the common model names', () => {
		expect(vendorOf('Claude Opus 4.5')).toBe('claude');
		expect(vendorOf('sonnet')).toBe('claude');
		expect(vendorOf('GPT-5')).toBe('openai');
		expect(vendorOf('gpt5')).toBe('openai');
		expect(vendorOf('o3-mini')).toBe('openai');
		expect(vendorOf('ChatGPT')).toBe('openai');
		expect(vendorOf('Gemini 2.5 Pro')).toBe('gemini');
		expect(vendorOf('Llama 3.1 70B')).toBe('meta');
		expect(vendorOf('Grok 4')).toBe('grok');
		expect(vendorOf('DeepSeek V3')).toBe('deepseek');
		expect(vendorOf('Mistral Large')).toBe('mistral');
		expect(vendorOf('Codestral')).toBe('mistral');
		expect(vendorOf('Qwen3-Coder')).toBe('qwen');
		expect(vendorOf('Kimi K2')).toBe('kimi');
		expect(vendorOf('GLM-4.6')).toBe('zhipu');
		expect(vendorOf('MiniMax M2')).toBe('minimax');
		expect(vendorOf('Gemma 3')).toBe('gemma');
		expect(vendorOf('Phi-4')).toBe('microsoft');
		expect(vendorOf('Amazon Nova Pro')).toBe('nova');
		expect(vendorOf('Command R+')).toBe('cohere');
		expect(vendorOf('Nemotron')).toBe('nvidia');
		expect(vendorOf('Granite 3')).toBe('ibm');
		expect(vendorOf('ERNIE 4.5')).toBe('wenxin');
		expect(vendorOf('GitHub Copilot (GPT-4o)')).toBe('openai');
		expect(vendorOf('GitHub Copilot')).toBe('githubcopilot');
		expect(vendorOf('Copilot')).toBe('copilot');
	});

	it('leaves the rest unknown', () => {
		expect(vendorOf('Cursor')).toBeNull();
		expect(vendorOf('Falcon 180B')).toBeNull();
		expect(vendorOf('my own model')).toBeNull();
		expect(vendorOf('o1o')).toBeNull();
		expect(vendorOf('Metallica')).toBeNull();
	});
});

describe('vendorIcons', () => {
	it('gives each vendor once, unknown models a robot', () => {
		expect(vendorIcons('Claude Opus 4.5, Claude Sonnet 4.5, GPT-5')).toEqual(['claude', 'openai']);
		expect(vendorIcons('Falcon, Cursor, Claude')).toEqual([null, 'claude']);
		expect(vendorIcons(null)).toEqual([null]);
		expect(vendorIcons(' , ')).toEqual([null]);
		expect(vendorIcons('Claude, GPT, Gemini, Llama')).toHaveLength(3);
	});
});

describe('llmLabel', () => {
	it('always says assisted or generated', () => {
		expect(llmLabel({ use: 'generated', model: 'Claude Opus 4.5' })).toBe(
			'LLM-generated: Claude Opus 4.5'
		);
		expect(llmLabel({ use: 'assisted' })).toBe('LLM-assisted (model not named)');
		expect(llmLabel({ use: 'none' })).toMatch(/No LLM/);
	});
});
