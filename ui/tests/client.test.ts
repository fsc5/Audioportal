import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { Api } from '../src/api.ts';
import { Controller } from '../src/controller.ts';

const fixture = async (name: string) => JSON.parse(await readFile(new URL(`../../specs/fixtures/${name}.json`, import.meta.url), 'utf8'));
const settings = await fixture('settings');
const stopped = await fixture('state-stopped');
const receiving = await fixture('state-receiving');
const devices = await fixture('devices');
const failure = await fixture('error');
const wait = (ms: number) => new Promise(resolve => setTimeout(resolve, ms));

function backend() {
  const calls: { path: string; method: string; token: string | null; body: unknown }[] = [];
  let saveError: unknown = null;
  let startError: unknown = null;
  let state = stopped;
  let token = 'launch-one';
  let stateFailure = false;
  const api = new Api(async (url, init) => {
    const path = String(url);
    const method = init?.method ?? 'GET';
    const sentToken = new Headers(init?.headers).get('X-Audioportal-Token');
    calls.push({ path, method, token: sentToken, body: init?.body ? JSON.parse(String(init.body)) : null });
    if (path === '/api/bootstrap') return Response.json({ csrf_token: token });
    if (method !== 'GET' && sentToken !== token) return Response.json({ error: { code: 'invalid_request', message: 'Missing or incorrect API token' } }, { status: 400 });
    if (path === '/api/settings' && method === 'PUT') return saveError ? Response.json(saveError, { status: 500 }) : Response.json(JSON.parse(String(init?.body)));
    if (path === '/api/start') {
      if (startError) return Response.json(startError, { status: 503 });
      state = receiving;
    }
    if (path === '/api/stop') state = stopped;
    if (path === '/api/state' && stateFailure) throw new TypeError('offline');
    return Response.json(path === '/api/settings' ? settings : path === '/api/devices' ? devices : state);
  });
  return { api, calls,
    failSave(value: unknown) { saveError = value; },
    failStart(value: unknown) { startError = value; },
    restart() { token = 'launch-two'; },
    offline() { stateFailure = true; },
  };
}

test('fixture flow saves before Start, propagates failures, and attaches launch token', async () => {
  const server = backend();
  const controller = new Controller(() => {}, server.api);
  try {
    await controller.initialize();
    await controller.initialize();
    assert.equal(server.calls.filter(c => c.path === '/api/bootstrap').length, 1);
    const draft = { ...settings, peer_ip: '192.168.1.20' };
    server.failSave(failure);
    await controller.mutate('start', draft);
    assert.equal(server.calls.filter(c => c.path === '/api/start').length, 0);
    assert.equal(controller.view.error, failure.error.message);
    assert.deepEqual(controller.view.saved, settings);
    server.failSave(null);
    server.failStart(failure);
    await controller.mutate('start', draft);
    assert.equal(controller.view.error, failure.error.message);
    assert.deepEqual(controller.view.saved, draft);
    server.failStart(null);
    await controller.mutate('start', draft);
    assert.deepEqual(controller.view.state, receiving);
    const mutations = server.calls.filter(c => c.method !== 'GET');
    assert.deepEqual(mutations.map(c => c.path), ['/api/settings', '/api/settings', '/api/start', '/api/settings', '/api/start']);
    assert.ok(mutations.every(c => c.token === 'launch-one'));
    assert.deepEqual(mutations.at(-2)?.body, draft);
    assert.equal(mutations.at(-1)?.body, null);
    await controller.mutate('save', settings);
    assert.equal(server.calls.filter(c => c.method !== 'GET').length, mutations.length);
    await controller.mutate('stop', draft);
    assert.deepEqual(controller.view.state, stopped);
    assert.equal(server.calls.filter(c => c.path === '/api/bootstrap').length, 1);
  } finally { controller.dispose(); }
});

test('restart refreshes once, preserves the failed operation, and requires an explicit retry', async () => {
  const server = backend();
  const controller = new Controller(() => {}, server.api);
  try {
    await controller.initialize();
    server.restart();
    await controller.mutate('start', settings);
    assert.match(controller.view.error, /token refreshed/);
    assert.equal(server.calls.filter(c => c.path === '/api/bootstrap').length, 2);
    assert.equal(server.calls.filter(c => c.path === '/api/start').length, 0);
    assert.equal(server.calls.filter(c => c.method === 'PUT').length, 1);
    await controller.mutate('start', settings);
    assert.deepEqual(controller.view.state, receiving);
    assert.equal(server.calls.findLast(c => c.path === '/api/start')?.token, 'launch-two');
  } finally { controller.dispose(); }
});

test('pending mutations cannot duplicate; slow polling does not overlap and is aborted on dispose', async () => {
  const server = backend();
  const controller = new Controller(() => {}, server.api);
  await controller.initialize();
  const original = server.api.fetcher;
  let release: (() => void) | undefined;
  server.api.fetcher = async (url, init) => {
    if (init?.method === 'PUT') await new Promise<void>(resolve => { release = resolve; });
    return original(url, init);
  };
  const saving = controller.mutate('save', settings);
  await controller.mutate('start', settings);
  release?.();
  await saving;
  assert.equal(server.calls.filter(c => c.method === 'PUT').length, 1);
  assert.equal(server.calls.filter(c => c.path === '/api/start').length, 0);
  let polls = 0;
  let aborted = false;
  server.api.fetcher = async (_url, init) => {
    polls++;
    return new Promise<Response>((_resolve, reject) => {
      init?.signal?.addEventListener('abort', () => { aborted = true; reject(new DOMException('Aborted', 'AbortError')); });
    });
  };
  await wait(2200);
  assert.equal(polls, 1);
  controller.dispose();
  assert.equal(aborted, true);
  await wait(1100);
  assert.equal(polls, 1);
});

test('poll failure removes stale success, while settings remain intact', async () => {
  const server = backend();
  const controller = new Controller(() => {}, server.api);
  try {
    await controller.initialize();
    await controller.mutate('start', settings);
    server.offline();
    await wait(1100);
    assert.equal(controller.view.online, false);
    assert.equal(controller.view.state, null);
    assert.deepEqual(controller.view.saved, settings);
  } finally { controller.dispose(); }
});

test('invalid successful responses are rejected at the API boundary', async () => {
  const api = new Api(async () => Response.json({ ...receiving, output_peak: 2 }));
  await assert.rejects(api.state(new AbortController().signal), /unexpected response/);
});

test('default fetch is called without binding it to the API instance', async () => {
  const original = globalThis.fetch;
  globalThis.fetch = async function () {
    assert.ok(this === undefined || this === globalThis);
    return Response.json(stopped);
  };
  try {
    assert.deepEqual(await new Api().state(new AbortController().signal), stopped);
  } finally { globalThis.fetch = original; }
});
