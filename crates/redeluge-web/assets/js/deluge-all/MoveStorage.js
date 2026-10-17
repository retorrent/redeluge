/**
 * Deluge.MoveStorage.js
 *
 * Copyright (c) Damien Churchill 2009-2010 <damoxc@gmail.com>
 *
 * This file is part of Deluge and is licensed under GNU General Public License 3.0, or later, with
 * the additional special exception to link portions of this program with the OpenSSL library.
 * See LICENSE for more details.
 *
 * The folders are on the daemon's disk, which this machine cannot see, so the
 * window browses them with `redeluge.list_directory`, as the create-torrent
 * window does. The path at the top is where the torrent goes: a click on a
 * folder puts it there, a double click opens it, and it can be typed into for
 * a folder that does not exist yet or a path pasted from somewhere else.
 */

Ext.namespace('Deluge');
Deluge.MoveStorage = Ext.extend(Ext.Window, {
    constructor: function (config) {
        config = Ext.apply(
            {
                title: _('Change Location'),
                width: 560,
                height: 400,
                minWidth: 380,
                minHeight: 260,
                layout: 'border',
                buttonAlign: 'right',
                closeAction: 'hide',
                closable: true,
                iconCls: 'x-deluge-move-storage',
                plain: true,
                constrainHeader: true,
            },
            config
        );
        Deluge.MoveStorage.superclass.constructor.call(this, config);
    },

    initComponent: function () {
        Deluge.MoveStorage.superclass.initComponent.call(this);

        this.addButton(_('Cancel'), this.onCancel, this);
        this.addButton(_('Move'), this.onMove, this);

        this.entries = new Ext.data.JsonStore({
            fields: ['name', 'path', 'kind'],
            root: 'entries',
        });

        this.browseField = new Ext.form.TextField({
            emptyText: _('Path on the daemon'),
            width: 360,
            enableKeyEvents: true,
            listeners: {
                specialkey: function (field, e) {
                    if (e.getKey() === e.ENTER) this.browse(field.getValue());
                },
                scope: this,
            },
        });

        this.upButton = new Ext.Button({
            text: _('Up'),
            iconCls: 'icon-up',
            handler: function () {
                if (this.parentPath) this.browse(this.parentPath);
            },
            scope: this,
        });

        this.listing = this.add({
            region: 'center',
            xtype: 'grid',
            store: this.entries,
            border: false,
            autoExpandColumn: 'name',
            selModel: new Ext.grid.RowSelectionModel({ singleSelect: true }),
            viewConfig: {
                emptyText: _('No folders in here.'),
                deferEmptyText: true,
            },
            columns: [
                {
                    id: 'name',
                    header: _('Folder'),
                    dataIndex: 'name',
                    sortable: false,
                    renderer: function (value) {
                        return String.format(
                            '<div class="x-deluge-browse-row x-deluge-browse-dir">{0}</div>',
                            Ext.util.Format.htmlEncode(value)
                        );
                    },
                },
            ],
            tbar: [
                this.upButton,
                ' ',
                this.browseField,
                ' ',
                {
                    text: _('Go'),
                    handler: function () {
                        this.browse(this.browseField.getValue());
                    },
                    scope: this,
                },
            ],
            listeners: {
                rowclick: function (grid, index) {
                    this.browseField.setValue(this.entries.getAt(index).get('path'));
                },
                rowdblclick: function (grid, index) {
                    this.browse(this.entries.getAt(index).get('path'));
                },
                scope: this,
            },
        });
    },

    /** Lists the folders of one directory on the daemon. */
    browse: function (path) {
        deluge.client.redeluge.list_directory(path || '/', {
            success: function (listing) {
                this.browseField.setValue(listing.path);
                this.parentPath = listing.parent;
                this.upButton.setDisabled(!listing.parent);
                // Only folders: a torrent is moved into one, never onto a file.
                var dirs = [];
                Ext.each(listing.entries || [], function (entry) {
                    if (entry.kind === 'dir') dirs.push(entry);
                });
                this.entries.loadData({ entries: dirs });
            },
            failure: function () {
                Ext.MessageBox.show({
                    title: _('Change Location'),
                    msg: String.format(
                        _('Could not read {0}'),
                        Ext.util.Format.htmlEncode(path)
                    ),
                    buttons: Ext.MessageBox.OK,
                    icon: Ext.MessageBox.WARNING,
                });
            },
            scope: this,
        });
    },

    /** Opens where the torrent is now, or where downloads go for several. */
    browseFromCurrent: function (torrentIds) {
        var start = function (path) {
            this.browse(path || '/');
        };
        if (torrentIds && torrentIds.length === 1) {
            deluge.client.web.get_torrent_status(torrentIds[0], ['save_path'], {
                success: function (status) {
                    start.call(this, status && status.save_path);
                },
                failure: function () {
                    start.call(this, '/');
                },
                scope: this,
            });
            return;
        }
        deluge.client.core.get_config_value('download_location', {
            success: function (location) {
                start.call(this, location);
            },
            failure: function () {
                start.call(this, '/');
            },
            scope: this,
        });
    },

    hide: function () {
        Deluge.MoveStorage.superclass.hide.call(this);
        this.torrentIds = null;
    },

    show: function (torrentIds) {
        Deluge.MoveStorage.superclass.show.call(this);
        this.torrentIds = torrentIds;
        this.browseFromCurrent(torrentIds);
    },

    onCancel: function () {
        this.hide();
    },

    onMove: function () {
        var dest = Ext.util.Format.trim(this.browseField.getValue());
        if (!dest) return;
        deluge.client.core.move_storage(this.torrentIds, dest);
        this.hide();
    },
});
deluge.moveStorage = new Deluge.MoveStorage();
