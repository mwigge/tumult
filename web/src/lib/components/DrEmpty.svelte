<script lang="ts">
    // The DR page of a lake that holds no DR test, no target with an RTO and
    // no DR plan yet: what a DR test is and where to start one.
    import { DR_GUIDE_URL, PLANS_HREF } from '#lib/dr.js';
</script>

<div class="panel empty">
    <p class="lead">No DR tests yet.</p>
    <p>
        A DR test is a run of kind <code>dr</code> with an RTO, or a DR exercise that people ran by hand:
        a failover, a restore, a backup or a drill.
    </p>
    <ul>
        <li>
            Write a DR experiment: read
            <a href={DR_GUIDE_URL} target="_blank" rel="noopener noreferrer">the DR guide</a>.
        </li>
        <li>Record a DR exercise you ran by hand on the <a href="/manual">Manual page</a>.</li>
        <li>Plan the next exercise with a due date under <a href={PLANS_HREF}>Plans</a>.</li>
    </ul>
</div>

<style>
    .empty {
        max-width: 720px;
        color: var(--text-dim);
    }
    .lead {
        color: var(--text);
        font-size: var(--font-size-xl);
        font-weight: var(--font-weight-strong);
        margin-top: 0;
    }
    ul {
        margin: 0;
        padding-left: var(--space-5);
    }
    li + li {
        margin-top: var(--space-1);
    }
</style>
