import { test, expect } from '@playwright/test';

test.beforeEach(async ({ page }) => {
  await page.addInitScript(() => {
    const responseByCommand: Record<string, unknown> = {
      list_saved_connections: [],
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

test('Authentication toggle shows and hides IAM fields', async ({ page }) => {
  await page.goto('/');

  await page.locator('.navigator .nav-header .icon-btn.header-icon[title="Add"]').click();
  await page.getByRole('menuitem', { name: 'New Connection' }).click();

  const dialog = page.getByRole('dialog', { name: 'Connection Manager' });
  await expect(dialog).toBeVisible();

  const authSelect = dialog.getByLabel('Authentication');
  const passwordInput = dialog.locator('input[type="password"]');

  // Password mode should show username/password and hide IAM fields.
  await expect(dialog.getByLabel('Username')).toBeVisible();
  await expect(passwordInput).toBeVisible();
  await expect(dialog.getByLabel('AWS Region')).toHaveCount(0);
  await expect(dialog.getByLabel('AWS Profile')).toHaveCount(0);
  await expect(dialog.getByLabel('TLS CA Bundle Path')).toHaveCount(0);

  await authSelect.selectOption('awsIam');

  // IAM mode should show IAM-specific fields and hide password field.
  await expect(dialog.getByLabel('AWS Region')).toBeVisible();
  await expect(dialog.getByLabel('AWS Profile')).toBeVisible();
  await expect(dialog.getByLabel('TLS CA Bundle Path')).toBeVisible();
  await expect(dialog.getByLabel('Database User')).toBeVisible();
  await expect(dialog.getByLabel('Username')).toHaveCount(0);
  await expect(passwordInput).toHaveCount(0);

  await authSelect.selectOption('password');

  // Switching back restores password auth fields.
  await expect(dialog.getByLabel('Username')).toBeVisible();
  await expect(passwordInput).toBeVisible();
  await expect(dialog.getByLabel('AWS Region')).toHaveCount(0);
  await expect(dialog.getByLabel('AWS Profile')).toHaveCount(0);
  await expect(dialog.getByLabel('TLS CA Bundle Path')).toHaveCount(0);
});
