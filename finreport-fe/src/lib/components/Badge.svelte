<script lang="ts">
	/**
	 * Generic text+shape badge/pill (§6 "never colour alone"). Reused across
	 * WP5 (label-source, needs-review) and WP6 (rule auto-approved, state).
	 * Keep the prop surface to `text`/`variant` so both packages can share it
	 * without either owning Svelte-specific rendering logic.
	 */
	interface Props {
		/** Visible label text — the only thing that conveys meaning (never rely on colour alone). */
		text: string;
		/** Selects both colour and a small leading glyph, so variants differ in shape too. */
		variant?: 'neutral' | 'info' | 'success' | 'warning';
		class?: string;
	}

	let { text, variant = 'neutral', class: className = '' }: Props = $props();

	const variants: Record<NonNullable<Props['variant']>, string> = {
		neutral: 'bg-slate-100 text-slate-700',
		info: 'bg-sky-100 text-sky-800',
		success: 'bg-emerald-100 text-emerald-800',
		warning: 'bg-amber-100 text-amber-800'
	};
	// A distinct leading glyph per variant so the badge's shape, not only its
	// colour, carries meaning.
	const glyphs: Record<NonNullable<Props['variant']>, string> = {
		neutral: '●',
		info: '◆',
		success: '✓',
		warning: '▲'
	};
</script>

<span
	class="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs font-medium {variants[
		variant
	]} {className}"
>
	<span aria-hidden="true">{glyphs[variant]}</span>
	{text}
</span>
