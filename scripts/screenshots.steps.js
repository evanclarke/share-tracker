// screenshots.steps.js — capture the README's screenshots, light and dark.
//
// Driven by scripts/screenshots.sh, which starts the server, seeds the
// `showcase` fixture and passes the output directory as the first argument.
// Run it through that script rather than directly; it assumes the fixture's
// data is already in place.
//
// Each shot is taken twice, once per colour scheme, because the README pairs
// them in a <picture> element so GitHub serves whichever matches the reader's
// own theme.

const THEME_KEY = 'share-tracker.theme';

// Wide enough that the Portfolio Overview graph draws its axis at full weekly
// density (it is built at its holder's *measured* width — see chart.js) and
// that the widest report table fits without its own horizontal scrollbar
// clipping a column mid-word.
const WIDTH = 1680;

// Each shot is cropped to its content rather than left on a fixed canvas: a
// one-row report on a 1000px page is mostly empty. The viewport is squeezed
// short first so `scrollHeight` reports the content's own height (the shell
// sets a 100vh minimum, which would otherwise just echo the viewport back),
// then reopened to it. MAX keeps a long table from becoming an unreadably
// tall PNG — it scrolls in the app, and the screenshot only has to show the
// shape of the screen.
const MIN_HEIGHT = 320;
const MAX_HEIGHT = 1400;

const SHOTS = [
  {
    name: 'overview',
    route: '#/r/overview',
    // The home screen: value graph, period-performance attribution, holdings.
    wait: 'document.querySelector("#app svg") && document.querySelectorAll("#app tbody tr").length > 0',
  },
  {
    name: 'net-capital-gain',
    route: '#/r/net-capital-gain',
    wait: 'document.querySelectorAll("#app tbody tr").length > 0',
  },
  {
    name: 'tax-summary',
    route: '#/r/tax-summary',
    wait: 'document.querySelectorAll("#app tbody tr").length > 0',
  },
  {
    name: 'open-parcels',
    route: '#/r/open-parcels',
    wait: 'document.querySelectorAll("#app tbody tr").length > 0',
  },
];

export default async function (page, args) {
  const out = args[0];
  if (!out) throw new Error('screenshots.steps.js: give an output directory');

  for (const theme of ['light', 'dark']) {
    // The scheme is read by the shell's pre-paint script at boot, so it has to
    // be stored and *then* loaded — setting it on a live page would leave the
    // first paint in the other scheme. Every shot after that is a hash route
    // within the loaded document (page.route), not a fresh load: navigating to
    // a hash is a same-document navigation, which fires no load event for
    // page.goto to wait on.
    await page.viewport(WIDTH, MIN_HEIGHT);
    await page.goto('/');
    await page.eval(`localStorage.setItem(${JSON.stringify(THEME_KEY)}, ${JSON.stringify(theme)})`);
    await page.goto('/');

    for (const shot of SHOTS) {
      await page.viewport(WIDTH, MIN_HEIGHT);
      await page.route(shot.route);
      await page.waitFor(shot.wait);
      await page.settle();

      const applied = await page.eval('document.documentElement.getAttribute("data-theme")');
      if (applied !== theme) throw new Error(`${shot.name}: expected ${theme}, page is ${applied}`);

      const content = await page.eval('document.documentElement.scrollHeight');
      const height = Math.max(MIN_HEIGHT, Math.min(MAX_HEIGHT, content));
      await page.viewport(WIDTH, height);
      await page.settle();

      const path = `${out}/${shot.name}-${theme}.png`;
      await page.screenshot(path);
      console.log(`${path} (${WIDTH}x${height})`);
    }
  }

  const errs = page.pageErrors();
  if (errs.length) throw new Error('page errors:\n' + errs.join('\n'));
}
