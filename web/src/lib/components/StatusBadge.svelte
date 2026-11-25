<script lang="ts">
    import Badge from './Badge.svelte';

    let { status }: { status: string | null } = $props();
</script>

<Badge kind="status" value={status} label={status ?? 'unknown'} />
