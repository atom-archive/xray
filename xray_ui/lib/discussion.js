const React = require("react");
const PropTypes = require("prop-types");
const { styled } = require("styletron-react");
const TextareaAutosize = require("react-autosize-textarea").default;
const $ = React.createElement;

const Root = styled("div", {
  width: "100%",
  height: "100%",
  paddingTop: "5px",
  paddingRight: "5px",
  paddingBottom: "5px",
  paddingLeft: 0,
  margin: 0,
  display: "flex",
  flexDirection: "column",
  boxSizing: "border-box",
  backgroundColor: "rgb(234, 234, 235)"
});

const Messages = styled("div", {
  flex: 1,
  display: "flex",
  flexDirection: "column",
  background: "white",
  marginBottom: "5px",
  overflowY: "auto",
  "::-webkit-scrollbar": {
    width: "5px"
  },
  "::-webkit-scrollbar-thumb": {
    borderRadius: "5px",
    background: "rgba(150, 150, 150, .33)"
  }
});

const Message = styled("div", ({ $first }) => ({
  padding: "5px",
  fontFamily: "Helvetica Neue",
  cursor: "default",
  overflowWrap: "break-word",
  marginTop: $first ? "auto" : null,
  ":hover": {
    background: "rgba(31, 150, 255, 0.3)"
  }
}));

const Avatar = styled("div", ({ $color }) => {
  const { r, g, b, a } = $color;
  return {
    backgroundColor: `rgba(${r}, ${g}, ${b}, ${a})`,
    display: "inline-block",
    width: "16px",
    height: "16px",
    borderRadius: "8px",
    position: "relative",
    top: "2px",
    marginRight: "4px"
  };
});

const TextArea = styled(TextareaAutosize, {
  padding: "5px",
  fontSize: "14px",
  resize: "none"
});

class Discussion extends React.Component {
  constructor() {
    super();
    this.handleKeyDown = this.handleKeyDown.bind(this);
  }

  componentDidMount() {
    this.scrollToBottom();
  }

  componentDidUpdate() {
    this.scrollToBottom();
  }

  scrollToBottom() {
    this.messages.scrollTop =
      this.messages.scrollHeight - this.messages.offsetHeight;
  }

  render() {
    const { userColors } = this.context.theme;
    return $(
      Root,
      null,
      $(
        Messages,
        {
          $ref: messages => {
            this.messages = messages;
          }
        },
        this.props.messages.map((message, i) => {
          const avatarColor = userColors[message.user_id % userColors.length];
          return $(
            Message,
            {
              $first: i === 0,
              onClick: () => this.jumpToAnchor(message.index)
            },
            $(Avatar, { $color: avatarColor }),
            message.text
          );
        })
      ),
      $(TextArea, {
        maxRows: 6,
        innerRef: textArea => (this.textArea = textArea),
        placeholder: "Message collaborators",
        onKeyDown: this.handleKeyDown
      })
    );
  }

  handleKeyDown(event) {
    if (event.key === "Enter") {
      if (this.textArea.value.length > 0) {
        this.props.dispatch({
          type: "Send",
          text: this.textArea.value
        });
        this.textArea.value = "";
      }
      event.preventDefault();
    }
  }

  jumpToAnchor(index) {
    this.props.dispatch({
      type: "Jump",
      message_index: index
    });
  }
}

Discussion.contextTypes = {
  theme: PropTypes.object
};

module.exports = Discussion;
