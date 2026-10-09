<script lang="ts">
    import { statusClass } from '#lib/api.js';

    let { status }: { status: string | null } = $props();
</script>

<span class="badge {statusClass(status)}">{status ?? 'unknown'}</span>
