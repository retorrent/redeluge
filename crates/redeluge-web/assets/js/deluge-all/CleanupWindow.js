/**
 * Deluge.CleanupWindow.js
 *
 * SPDX-License-Identifier: GPL-3.0-or-later
 *
 * What is in a directory that no torrent accounts for, to delete.
 *
 * For purging a disk after something went wrong: a torrent removed without
 * its data, a move that stopped halfway. One directory, not recursive. The
 * daemon decides what is claimed again when it deletes, so a torrent added
 * after the listing keeps its files: `redeluge.get_orphans` and
 * `redeluge.delete_orphans`.
 */
Ext.ns('Deluge');

/**
 * @class Deluge.CleanupWindow
 * @extends Ext.Window
 */
Deluge.CleanupWindow = Ext.extend(Ext.Window, {
    title: _('Cleanup'),
    width: 620,
    height: 420,
    layout: 'border',
    closeAction: 'hide',
    constrainHeader: true,
    plain: true,
    minWidth: 420,
    minHeight: 260,

    initComponent: function () {
        Deluge.CleanupWindow.superclass.initComponent.call(this);

        this.path = this.add({
            region: 'north',
            xtype: 'form',
            height: 36,
            border: false,
            bodyStyle: 'padding: 6px',
            labelWidth: 60,
            items: [
                {
                    xtype: 'textfield',
                    fieldLabel: _('Path'),
                    anchor: '-8',
                    enableKeyEvents: true,
                    listeners: {
                        specialkey: function (field, e) {
                            if (e.getKey() === e.ENTER) this.load();
                        },
                        scope: this,
                    },
                },
            ],
        }).items.get(0);

        this.store = new Ext.data.ArrayStore({
            fields: [
                { name: 'name', type: 'string' },
                { name: 'kind', type: 'string' },
                { name: 'size', type: 'int' },
            ],
        });
        this.selection = new Ext.grid.CheckboxSelectionModel();

        this.grid = this.add({
            region: 'center',
            xtype: 'grid',
            store: this.store,
            sm: this.selection,
            border: false,
            autoExpandColumn: 'name',
            viewConfig: {
                emptyText: _('Nothing here that no torrent accounts for.'),
                deferEmptyText: true,
            },
            columns: [
                this.selection,
                {
                    id: 'name',
                    header: _('Name'),
                    dataIndex: 'name',
                    renderer: function (value, meta, record) {
                        var icon = record.get('kind') == 'dir' ? '/' : '';
                        return Ext.util.Format.htmlEncode(value + icon);
                    },
                },
                {
                    header: _('Size'),
                    dataIndex: 'size',
                    width: 90,
                    renderer: fsize,
                },
            ],
        });

        this.addButton(_('List'), this.load, this);
        this.addButton(_('Delete selected'), this.onDelete, this);
        this.addButton(_('Close'), this.onClose, this);

        this.on('show', this.onShown, this);
    },

    onShown: function () {
        if (this.path.getValue()) return;
        // Where downloads go is where leftovers usually are.
        deluge.client.core.get_config_value('download_location', {
            success: function (location) {
                if (!this.path.getValue()) this.path.setValue(location || '');
            },
            scope: this,
        });
    },

    onClose: function () {
        this.hide();
    },

    load: function () {
        var path = this.path.getValue();
        if (!path) return;
        this.store.removeAll();
        this.grid.getEl().mask(_('Looking...'));
        deluge.client.redeluge.get_orphans(path, {
            success: function (entries) {
                this.grid.getEl().unmask();
                var rows = [];
                Ext.each(entries || [], function (entry) {
                    rows.push([entry['name'], entry['kind'], entry['size']]);
                });
                this.store.loadData(rows);
            },
            failure: function (response) {
                var error = response && response.error;
                this.grid.getEl().unmask();
                Ext.MessageBox.alert(
                    _('Cleanup'),
                    Ext.util.Format.htmlEncode(
                        (error && error.message) || _('Could not list it.')
                    )
                );
            },
            scope: this,
        });
    },

    onDelete: function () {
        var path = this.path.getValue();
        var names = [];
        Ext.each(this.selection.getSelections(), function (record) {
            names.push(record.get('name'));
        });
        if (!path || !names.length) return;

        Ext.MessageBox.confirm(
            _('Delete'),
            String.format(
                _('Delete {0} item(s) from {1}? This cannot be undone.'),
                names.length,
                Ext.util.Format.htmlEncode(path)
            ),
            function (answer) {
                if (answer != 'yes') return;
                deluge.client.redeluge.delete_orphans(path, names, {
                    success: function (failed) {
                        if (failed && failed.length) {
                            var lines = [];
                            Ext.each(failed, function (failure) {
                                lines.push(
                                    Ext.util.Format.htmlEncode(
                                        failure['name'] + ': ' + failure['error']
                                    )
                                );
                            });
                            Ext.MessageBox.alert(
                                _('Not deleted'),
                                lines.join('<br>')
                            );
                        }
                        this.load();
                    },
                    failure: function (response) {
                        var error = response && response.error;
                        Ext.MessageBox.alert(
                            _('Cleanup'),
                            Ext.util.Format.htmlEncode(
                                (error && error.message) ||
                                    _('Could not delete them.')
                            )
                        );
                    },
                    scope: this,
                });
            },
            this
        );
    },
});
