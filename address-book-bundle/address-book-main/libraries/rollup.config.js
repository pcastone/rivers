import svelte from 'rollup-plugin-svelte';
import resolve from '@rollup/plugin-node-resolve';
import css from 'rollup-plugin-css-only';

export default {
  input: 'src/main.js',
  output: {
    file: 'spa/bundle.js',
    format: 'iife',
    name: 'app'
  },
  plugins: [
    svelte({ compilerOptions: { dev: false } }),
    css({ output: 'bundle.css' }),
    resolve({ browser: true, dedupe: ['svelte'] })
  ]
};
