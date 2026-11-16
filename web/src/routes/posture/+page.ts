// The Posture page is now called Resilience. Old links and bookmarks still
// work: the load runs in the browser (the console is a client-rendered SPA,
// prerendered as an empty shell), so it sees the real query string and keeps it.
import { redirect, type Load } from '@sveltejs/kit';

export const load: Load = ({ url }) => {
    redirect(308, `/resilience${url.search}`);
};
