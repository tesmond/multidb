import { test, expect } from '@playwright/test';

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
      ],
      load_schema: {
        schemaJson: JSON.stringify({
          tables: [
            {
              name: 'users',
              sizeBytes: 128,
              columns: [{ name: 'id', type: 'integer', key: 'PRI' }],
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

test('navigator renders tables with a single row element per table entry', async ({ page }) => {
  await page.goto('/');

  await expect(page.locator('.navigator')).toBeVisible();
  await expect(page.locator('.navigator .conn-label')).toContainText('Test DB');

  await page.locator('.navigator .conn-label').filter({ hasText: 'Test DB' }).click();
  await page.locator('.navigator .section-label').filter({ hasText: 'Tables' }).click();

  await expect(page.locator('.navigator .table-label')).toContainText('users');
  await page.locator('.navigator .table-label').filter({ hasText: 'users' }).click();
  await expect(page.locator('.navigator .col-row')).toContainText('id');
});
