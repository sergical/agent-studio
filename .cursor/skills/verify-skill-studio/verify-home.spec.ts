import { test, expect } from '@playwright/test';

test.describe('Skill Studio - Home Dashboard Verification', () => {
  test('home dashboard loads and displays', async ({ page }) => {
    // Configure longer timeout for initial load
    test.setTimeout(60000);

    // Navigate to the Tauri dev server
    await page.goto('http://localhost:1420', { timeout: 30000 });
    
    // Wait for the app to be ready - look for the sidebar
    try {
      await page.waitForSelector('text=Home', { timeout: 15000 });
    } catch (error) {
      // If sidebar didn't load, capture what we see
      await page.screenshot({ 
        path: '/workspace/.cursor/skills/verify-skill-studio/evidence/error-page-load.png',
        fullPage: true 
      });
      throw new Error(`Failed to load app: ${error}`);
    }

    // Click Home to ensure we're on the home view
    const homeNavLink = page.locator('text=Home').first();
    if (await homeNavLink.isVisible()) {
      await homeNavLink.click();
      await page.waitForTimeout(1000);
    }
    
    // Take full page screenshot of home dashboard
    await page.screenshot({ 
      path: '/workspace/.cursor/skills/verify-skill-studio/evidence/home-dashboard.png',
      fullPage: true 
    });

    // Verify we can see some expected UI elements
    // The home view should have the sidebar visible - check for at least one navigation item
    const homeLink = page.locator('text=Home').first();
    const skillsLink = page.locator('text=Skills').first();
    const activityLink = page.locator('text=Activity').first();
    
    const sidebarVisible = (await homeLink.isVisible()) || 
                          (await skillsLink.isVisible()) || 
                          (await activityLink.isVisible());
    expect(sidebarVisible).toBeTruthy();

    // Log success
    console.log('✓ Home dashboard loaded successfully');
    console.log('✓ Screenshot captured: evidence/home-dashboard.png');

    // Try to capture browser console errors
    page.on('console', msg => {
      if (msg.type() === 'error') {
        console.error('Browser console error:', msg.text());
      }
    });
  });
});
