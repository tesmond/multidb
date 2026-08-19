<script lang="ts">
    import {
        showConnectionDialog,
        editingConnection,
        tabs,
        activeTabId,
        selectedConnId,
    } from "../stores/appStore";
    import { get } from "svelte/store";

    function newTab() {
        const connId = get(selectedConnId);
        tabs.add(connId);
        const allTabs = get(tabs);
        activeTabId.set(allTabs[allTabs.length - 1].id);
    }

    function newConnection() {
        editingConnection.set(null);
        showConnectionDialog.set(true);
    }
</script>
