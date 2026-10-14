import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import { chromium } from 'playwright';
import { createServer } from 'vite';

const fixture = async name => JSON.parse(await readFile(new URL(`../../specs/fixtures/${name}.json`, import.meta.url), 'utf8'));

test('collapsed invalid advanced fields are revealed and focused before either submission', async () => {
  const server = await createServer();
  let browser;
  try {
    await server.listen();
    browser = await chromium.launch({ executablePath: chromium.executablePath(), headless: true });
    const page = await browser.newPage();
    const settings = await fixture('settings');
    const devices = await fixture('devices');
    const stopped = await fixture('state-stopped');
    const mutations = [];
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    page.on('console', message => { if (message.type() === 'error') errors.push(message.text()); });
    await page.route('**/api/**', async route => {
      const request = route.request();
      const path = new URL(request.url()).pathname;
      if (request.method() !== 'GET') mutations.push(path);
      const body = path === '/api/bootstrap' ? { csrf_token: 'browser-test' }
        : path === '/api/settings' ? request.method() === 'PUT' ? request.postDataJSON() : settings
        : path === '/api/devices' ? devices : stopped;
      await route.fulfill({ json: body });
    });
    // Ignore the browser's optional favicon request in console-error assertions.
    await page.route('**/favicon.ico', route => route.fulfill({ status: 204 }));
    await page.goto('http://127.0.0.1:5173');
    const details = page.locator('details');
    const summary = page.getByText('Advanced settings', { exact: true });
    for (const button of ['Save', 'Save & Start']) {
      for (const [label, values, valid] of [
        ['UDP port', ['0', '65536', '1.5', ''], '46000'],
        ['Receive buffer (ms)', ['5', '105', '31', ''], '30'],
      ]) {
        const input = page.getByLabel(label, { exact: true });
        for (const invalid of values) {
          if (!await details.evaluate(el => el.open)) await summary.click();
          await input.fill(invalid);
          await summary.click();
          assert.equal(await details.evaluate(el => el.open), false);
          const before = mutations.length;
          await page.getByRole('button', { name: button, exact: true }).click();
          assert.equal(await details.evaluate(el => el.open), true, `${label}=${JSON.stringify(invalid)} via ${button}`);
          assert.equal(await input.isVisible(), true);
          assert.equal(await input.evaluate(el => document.activeElement === el), true);
          assert.notEqual(await input.evaluate(el => el.validationMessage), '');
          assert.equal(mutations.length, before, 'Invalid settings must not reach the API');
          await input.fill(valid);
          await summary.click();
          await page.getByRole('button', { name: button, exact: true }).click();
          await page.waitForFunction(() => !document.querySelector('fieldset').disabled);
          assert.deepEqual(mutations.slice(before), button === 'Save' ? ['/api/settings'] : ['/api/settings', '/api/start']);
        }
      }
    }
    assert.deepEqual(errors, []);
  } finally {
    await browser?.close();
    await server.close();
  }
});
