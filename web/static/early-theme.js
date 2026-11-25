// Puts the theme the user saved on <html> before the first paint, so that
// a light page never shows the dark default first. The layout starts from
// it. app.html loads it as a file, not as inline code, for a strict
// content security policy.
try {
    const saved = localStorage.getItem('tumult.theme');
    if (saved === 'dark' || saved === 'light' || saved === 'auto') {
        document.documentElement.dataset.theme = saved;
    }
} catch {
    // No storage, as in a private window: the default theme stays.
}
