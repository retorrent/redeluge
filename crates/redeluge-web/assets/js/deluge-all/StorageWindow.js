/**
 * Deluge.StorageWindow.js
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Every place torrents live and what is left on it: each local filesystem the
 * downloads use, and each archive target with its rclone cache. Opened from
 * the free-space button in the status bar, which only ever showed the
 * download folder.
 *
 * The figures come from `redeluge.get_storage`.
 */
Ext.ns('Deluge');

/**
 * @class Deluge.StorageWindow
 * @extends Ext.Window
 */
Deluge.StorageWindow = Ext.extend(Ext.Window, {
    title: _('Storage'),
    width: 900,
    height: 320,
    layout: 'fit',
    closeAction: 'hide',
    constrainHeader: true,
    plain: true,
    minWidth: 560,
    minHeight: 200,

    /** How often it refreshes while open, in seconds. */
    interval: 10,

    initComponent: function () {
        Deluge.StorageWindow.superclass.initComponent.call(this);

        this.store = new Ext.data.JsonStore({
            fields: [
                'kind',
                'name',
                'path',
                'paths',
                'online',
                { name: 'free', type: 'float' },
                { name: 'total', type: 'float' },
                { name: 'torrents', type: 'int' },
                { name: 'torrent_bytes', type: 'float' },
                { name: 'cache_used', type: 'float' },
                { name: 'uploads_pending', type: 'int' },
            ],
        });

        this.add({
            xtype: 'grid',
            store: this.store,
            border: false,
            autoExpandColumn: 'storage-path',
            viewConfig: {
                emptyText: _('Nothing to show yet.'),
                deferEmptyText: false,
            },
            columns: [
                {
                    header: _('Media'),
                    dataIndex: 'name',
                    width: 130,
                    renderer: this.renderMedia,
                },
                {
                    id: 'storage-path',
                    header: _('Path'),
                    dataIndex: 'paths',
                    renderer: this.renderPaths,
                },
                {
                    header: _('Status'),
                    dataIndex: 'online',
                    width: 70,
                    renderer: this.renderStatus,
                },
                {
                    header: _('Free'),
                    dataIndex: 'free',
                    width: 230,
                    renderer: this.renderFree,
                },
                {
                    header: _('Torrents'),
                    dataIndex: 'torrents',
                    width: 110,
                    renderer: this.renderTorrents,
                },
                {
                    header: _('Cache'),
                    dataIndex: 'cache_used',
                    width: 75,
                    renderer: this.renderBytes,
                },
                {
                    header: _('Uploads'),
                    dataIndex: 'uploads_pending',
                    width: 70,
                    renderer: this.renderCount,
                },
            ],
        });

        this.addButton(_('Refresh'), this.load, this);
        this.addButton(_('Close'), this.hide, this);

        this.on('show', this.onShown, this);
        this.on('hide', this.stopPolling, this);
        this.on('destroy', this.stopPolling, this);
    },

    onShown: function () {
        this.load();
        this.stopPolling();
        this.timer = window.setInterval(
            this.load.createDelegate(this),
            this.interval * 1000
        );
    },

    stopPolling: function () {
        if (!this.timer) return;
        window.clearInterval(this.timer);
        this.timer = null;
    },

    load: function () {
        deluge.client.redeluge.get_storage({
            success: function (rows) {
                if (!this.isVisible()) return;
                this.store.loadData(rows || []);
            },
            failure: function () {
                if (!this.isVisible()) return;
                this.store.removeAll();
            },
            scope: this,
        });
    },

    renderMedia: function (value, p, r) {
        var archive = r.data.kind === 'archive';
        return String.format(
            '<div class="x-deluge-storage-media x-deluge-{0}">{1}</div>',
            archive
                ? r.data.online
                    ? 'remoteseed'
                    : 'remoteerror'
                : 'freespace',
            Ext.util.Format.htmlEncode(
                archive ? value || '' : _('Local disk')
            )
        );
    },

    renderPaths: function (paths) {
        return Ext.util.Format.htmlEncode((paths || []).join(', '));
    },

    renderStatus: function (online, p, r) {
        if (online) return _('Online');
        return String.format(
            '<span style="color: #f0883e; font-weight: bold;">{0}</span>',
            r.data.kind === 'archive' ? _('Offline') : _('Error')
        );
    },

    /**
     * Free of total, with the share in use as a bar. A remote with no quota
     * reports a petabyte or so through rclone; that is not a size, it is
     * "no limit".
     */
    renderFree: function (free, p, r) {
        var total = r.data.total;
        if (free < 0 || total <= 0) return '';
        if (total >= Math.pow(2, 50)) return _('No limit');
        var used = 100 - (free / total) * 100;
        return Deluge.progressBar(
            used,
            Deluge.columnWidth(p, 230) - 8,
            String.format(_('{0} free of {1}'), fsize(free), fsize(total))
        );
    },

    renderTorrents: function (count, p, r) {
        if (!count) return '0';
        return String.format('{0} · {1}', count, fsize(r.data.torrent_bytes));
    },

    renderBytes: function (value) {
        return value >= 0 ? fsize(value) : '';
    },

    renderCount: function (value) {
        return value >= 0 ? String(value) : '';
    },
});
