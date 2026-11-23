<script lang="ts">
    // Who ran the test, and whether from inside the entity or from outside
    // it. DORA Art. 24(4) asks that tests are run by independent parties.
    import { TESTER_KINDS, type ManualForm } from '#lib/manual.js';

    let { form = $bindable() }: { form: ManualForm } = $props();
</script>

<div class="form">
    <div class="row2">
        <label
            >Tester <input
                type="text"
                placeholder="person, team or firm"
                bind:value={form.tester_name}
            /></label
        >
        <label
            >Tester kind
            <select bind:value={form.tester_kind}>
                <option value="">not set</option>
                {#each TESTER_KINDS as k (k)}<option value={k}>{k}</option>{/each}
            </select>
        </label>
    </div>
    <p class="note">
        An internal tester is a team of the entity that is independent of the system it tests. An
        external tester comes from outside the entity.
    </p>
</div>
