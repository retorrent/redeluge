/**
 * Deluge.BannedWindow.js
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * Torrents that are not to be downloaded, and banning more.
 *
 * Two ways onto the list: Blocklist on a torrent's menu, which bans it, tells
 * the *arr of its label and deletes it with its files; and a tracker blocked
 * in its settings, whose torrents are held, paused and in error, and nothing
 * deleted. The list keeps when each was banned and its name once known, and
 * what the *arr said. `redeluge.get_banned` and friends; see `banned.rs`.
 */
Ext.ns('Deluge');

/**
 * @class Deluge.BannedWindow
 * @extends Ext.Window
 */
Deluge.BannedWindow = Ext.extend(Ext.Window, {
    title: _('Banned Torrents'),
    width: 820,
    height: 420,
    layout: 'fit',
    closeAction: 'hide',
    constrainHeader: true,
    plain: true,
    minWidth: 480,
    minHeight: 260,

    initComponent: function () {
        Deluge.BannedWindow.superclass.initComponent.call(this);

        this.store = new Ext.data.JsonStore({
            fields: [
                'id',
                { name: 'at', type: 'float' },
                'name',
                'tracker',
                'label',
                'reason',
                'in_session',
                'arr_state',
                'arr_message',
            ],
        });
        this.selection = new Ext.grid.CheckboxSelectionModel();

        this.grid = this.add({
            xtype: 'grid',
            store: this.store,
            sm: this.selection,
            border: false,
            autoExpandColumn: 'name',
            viewConfig: {
                emptyText: _('Nothing is banned.'),
                deferEmptyText: false,
            },
            columns: [
                this.selection,
                {
                    header: _('Banned'),
                    dataIndex: 'at',
                    width: 120,
                    renderer: fdate,
                },
                {
                    id: 'name',
                    header: _('Name'),
                    dataIndex: 'name',
                    renderer: function (value, meta, record) {
                        meta.attr =
                            'ext:qtip="' +
                            Ext.util.Format.htmlEncode(record.get('id')) +
                            '"';
                        return Ext.util.Format.htmlEncode(value);
                    },
                },
                {
                    header: _('Tracker'),
                    dataIndex: 'tracker',
                    width: 110,
                    renderer: Ext.util.Format.htmlEncode,
                },
                {
                    header: _('Label'),
                    dataIndex: 'label',
                    width: 70,
                    renderer: Ext.util.Format.htmlEncode,
                },
                {
                    header: _('Why'),
                    dataIndex: 'reason',
                    width: 150,
                    renderer: Ext.util.Format.htmlEncode,
                },
                {
                    header: _('*arr'),
                    dataIndex: 'arr_state',
                    width: 120,
                    renderer: function (value, meta, record) {
                        var message = record.get('arr_message');
                        if (message) {
                            meta.attr =
                                'ext:qtip="' +
                                Ext.util.Format.htmlEncode(message) +
                                '"';
                        }
                        return Ext.util.Format.htmlEncode(
                            Deluge.BannedWindow.ARR[value] || value
                        );
                    },
                },
            ],
        });

        this.addButton(_('Send to *arr'), this.onSend, this);
        this.addButton(_('Unban'), this.onUnban, this);
        this.addButton(_('Refresh'), this.load, this);
        this.addButton(_('Close'), this.onClose, this);

        this.on('show', this.load, this);
    },

    onClose: function () {
        this.hide();
    },

    load: function () {
        deluge.client.redeluge.get_banned({
            success: function (entries) {
                if (!this.isVisible()) return;
                this.store.loadData(entries || []);
            },
            scope: this,
        });
    },

    selected: function () {
        var ids = [];
        Ext.each(this.selection.getSelections(), function (record) {
            ids.push(record.get('id'));
        });
        return ids;
    },

    onSend: function () {
        var ids = this.selected();
        if (!ids.length) return;
        deluge.client.redeluge.send_banned(ids, {
            success: function (answers) {
                Deluge.BannedWindow.report(answers);
                this.load();
            },
            failure: Deluge.BannedWindow.failed,
            scope: this,
        });
    },

    onUnban: function () {
        var ids = this.selected();
        if (!ids.length) return;
        deluge.client.redeluge.unban_torrents(ids, {
            success: this.load,
            failure: Deluge.BannedWindow.failed,
            scope: this,
        });
    },
});

/** What each *arr state says, in words. */
Deluge.BannedWindow.ARR = {
    off: '—',
    pending: _('Waiting'),
    blocklisted: _('Blocklisted'),
    not_in_queue: _('Not in its queue'),
    failed: _('Failed'),
};

/**
 * Bans torrents from their menu: confirm, ban, tell the *arr, delete.
 */
Deluge.BannedWindow.ban = function (ids) {
    if (!ids || !ids.length) return;
    Ext.MessageBox.confirm(
        _('Blocklist'),
        String.format(
            _(
                'Ban {0} torrent(s)? They are deleted with their files, the *arr of their label is told if it is set up to be, and they are held if they are ever added again.'
            ),
            ids.length
        ),
        function (answer) {
            if (answer != 'yes') return;
            deluge.client.redeluge.ban_torrents(ids, '', true, {
                success: function (answers) {
                    Deluge.BannedWindow.report(answers);
                    deluge.ui.update();
                },
                failure: Deluge.BannedWindow.failed,
            });
        }
    );
};

/**
 * Says what the *arr answered, when anything was asked of one.
 */
Deluge.BannedWindow.report = function (answers) {
    var lines = [];
    Ext.each(answers || [], function (answer) {
        var arr = answer['arr'];
        if (!arr) return;
        var said = Deluge.BannedWindow.ARR[arr['state']] || arr['state'];
        if (arr['message']) said += ': ' + arr['message'];
        lines.push(
            Ext.util.Format.htmlEncode(
                String(answer['id']).substr(0, 8) + '… ' + said
            )
        );
    });
    if (lines.length) {
        Ext.MessageBox.alert(_('*arr'), lines.join('<br>'));
    }
};

Deluge.BannedWindow.failed = function (response) {
    var error = response && response.error;
    Ext.MessageBox.alert(
        _('Blocklist'),
        Ext.util.Format.htmlEncode(
            (error && error.message) || _('The daemon did not do it.')
        )
    );
};
