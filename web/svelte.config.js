import adapter from '@sveltejs/adapter-static';

export default {
  kit: {
    // Pure SPA: static prerendered shells + a fallback for client-side
    // routes (embedded into kronikad via rust-embed).
    adapter: adapter({ fallback: '200.html' })
  }
};
