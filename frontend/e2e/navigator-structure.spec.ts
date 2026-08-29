import { test, expect, type Page } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    (window as any).__commands = [];
    const responseByCommand: Record<string, unknown> = {
      list_saved_connections: [
        {
          id: 'conn-1',
          name: 'Test DB',
          driver: 'postgres',
          host: 'localhost',
          port: 5432,
          database: 'app',
          username: 'user',
          password: 'pass',
        },
        {
          id: 'conn-2',
          name: 'Replica DB',
          driver: 'postgres',
          host: 'replica.localhost',
          port: 5432,
          database: 'app',
          username: 'user',
          password: 'pass',
        },
        {
          id: 'conn-3',
          name: 'Local DB',
          driver: 'sqlite',
          host: '',
          port: 0,
          database: 'local.db',
          username: '',
          password: '',
        },
      ],
      load_schema: {
        schemaJson: JSON.stringify({
          tables: [
            {
              name: 'users',
              sizeBytes: 128,
              columns: [
                { name: 'id', type: 'integer', key: 'PRI' },
                { name: 'email_address', type: 'text', key: '' },
              ],
            },
          ],
          views: [],
          indexes: [],
        }),
      },
      get_saved_queries: [],
      get_query_history: [],
      get_query_history_by_conn_id: [],
      list_connections: [],
    };

    (window as any).ipc = {
      postMessage: (raw: string) => {
        let message: { id: string; command: string } | null = null;
        try {
          message = JSON.parse(raw);
        } catch {
          return;
        }
        (window as any).__commands.push(message.command);
        const payload = Object.prototype.hasOwnProperty.call(responseByCommand, message.command)
          ? responseByCommand[message.command]
          : null;

        setTimeout(() => {
          (window as any).__MULTIDB__?.resolve?.(message!.id, payload);
        }, 0);
      },
    };
  });
});

async function seedProductionGroup(page: Page) {
  await page.addInitScript(() => {
    localStorage.setItem(
      'multidb.serverGroups.v1',
      JSON.stringify([
        {
          id: 'production',
          title: 'Production',
          connectionIds: ['conn-2', 'conn-1'],
        },
      ]),
    );
  });
}

test('navigator renders tables with a single row element per table entry', async ({ page }) => {
  await page.goto('/');

  await expect(page.locator('.navigator')).toBeVisible();
  await expect(page.locator('.navigator .conn-label').filter({ hasText: 'Test DB' })).toBeVisible();

  await page.locator('.navigator .conn-label').filter({ hasText: 'Test DB' }).click();
  await page.locator('.navigator .section-label').filter({ hasText: 'Tables' }).click();

  await expect(page.locator('.navigator .table-label')).toContainText('users');
  await page.locator('.navigator .table-label').filter({ hasText: 'users' }).click();

  const primaryColumn = page.locator('.navigator .col-row').filter({ hasText: 'id' });
  const regularColumn = page.locator('.navigator .col-row').filter({ hasText: 'email_address' });
  await expect(primaryColumn).toContainText('id');
  await expect(primaryColumn.locator('.col-key')).toHaveText('🔑');
  await expect(regularColumn.locator('.col-key')).toHaveCount(0);

  const primaryName = await primaryColumn.locator('.col-name').boundingBox();
  const regularName = await regularColumn.locator('.col-name').boundingBox();
  expect(primaryName).not.toBeNull();
  expect(regularName).not.toBeNull();
  expect(primaryName!.x).toBe(regularName!.x);
  await expect(primaryColumn.locator('.col-name + .col-key')).toHaveText('🔑');
});

test('connection dropdown follows navigator group order and labels', async ({ page }) => {
  await seedProductionGroup(page);
  await page.goto('/');

  const selector = page.getByRole('button', { name: 'Connection selector' });
  await selector.click();

  await expect(page.getByRole('option').locator('.option-label')).toHaveText([
    '— select connection —',
    'Production - Replica DB',
    'Production - Test DB',
    'Local DB',
  ]);
});

test('database sections are indented below grouped connections', async ({ page }) => {
  await seedProductionGroup(page);
  await page.goto('/');

  await page.locator('.navigator .group-label').filter({ hasText: 'Production' }).click();
  const connection = page.locator('.navigator .conn-label').filter({ hasText: 'Replica DB' });
  await connection.click();
  const databaseSection = page.locator('.navigator .section-label').filter({ hasText: 'Tables' });
  await expect(databaseSection).toBeVisible();

  const connectionChevron = await connection.locator('.chevron').boundingBox();
  const databaseChevron = await databaseSection.locator('.chevron').boundingBox();

  expect(connectionChevron).not.toBeNull();
  expect(databaseChevron).not.toBeNull();
  expect(databaseChevron!.x).toBeGreaterThan(connectionChevron!.x);
});

test('connection context menu opens an empty query for that connection', async ({ page }) => {
  await seedProductionGroup(page);
  await page.goto('/');

  await page.locator('.navigator .group-label').filter({ hasText: 'Production' }).click();
  await page.locator('.navigator .conn-label').filter({ hasText: 'Replica DB' }).click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Query', exact: true }).click();

  await expect(page.locator('.tab.active .tab-title')).toHaveText('Query');
  await expect(page.getByRole('button', { name: 'Connection selector' })).toContainText('Production - Replica DB');
  await expect(page.getByRole('button', { name: '💾 Save' })).toBeDisabled();
});
